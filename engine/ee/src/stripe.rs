use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::Value;

pub const TRIAL_DAYS: u64 = 30;
/// Pinned so an account-default upgrade cannot change the fields this client parses.
/// Webhook endpoints in the Dashboard must use this same version.
pub const API_VERSION: &str = "2026-09-30.endive";
const DEFAULT_API_BASE: &str = "https://api.stripe.com";

#[derive(Clone)]
pub struct StripeConfig {
    pub secret_key: String,
    pub price_id: String,
    pub webhook_secret: String,
    pub api_base: String,
}

impl StripeConfig {
    /// `None` when Stripe is unset, so a WorkOS engine can run without billing.
    /// A partial set is a configuration error.
    pub fn from_env() -> Result<Option<StripeConfig>, String> {
        let get = |name: &str| {
            std::env::var(name)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        let secret_key = get("STRIPE_SECRET_KEY");
        let price_id = get("STRIPE_PRICE_ID");
        let webhook_secret = get("STRIPE_WEBHOOK_SECRET");
        if secret_key.is_none() && price_id.is_none() && webhook_secret.is_none() {
            return Ok(None);
        }
        let secret_key =
            secret_key.ok_or("missing required environment variable: STRIPE_SECRET_KEY")?;
        let price_id = price_id.ok_or("missing required environment variable: STRIPE_PRICE_ID")?;
        let webhook_secret =
            webhook_secret.ok_or("missing required environment variable: STRIPE_WEBHOOK_SECRET")?;
        let api_base = get("STRIPE_API_BASE").unwrap_or_else(|| DEFAULT_API_BASE.to_string());
        reqwest::Url::parse(&api_base)
            .map_err(|err| format!("STRIPE_API_BASE is not a valid URL: {err}"))?;
        Ok(Some(StripeConfig {
            secret_key,
            price_id,
            webhook_secret,
            api_base: api_base.trim_end_matches('/').to_string(),
        }))
    }
}

#[derive(Debug)]
pub enum StripeError {
    Api(String),
    Unavailable,
}

#[derive(Debug, Deserialize)]
pub struct Subscription {
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub trial_end: Option<i64>,
    #[serde(default)]
    pub default_payment_method: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Customer {
    #[serde(default)]
    invoice_settings: InvoiceSettings,
}

#[derive(Debug, Default, Deserialize)]
struct InvoiceSettings {
    #[serde(default)]
    default_payment_method: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IdBody {
    id: String,
}

#[derive(Debug, Deserialize)]
struct PortalBody {
    url: String,
}

#[derive(Debug, Default, Deserialize)]
struct ErrorEnvelope {
    error: Option<ErrorDetail>,
}

#[derive(Debug, Default, Deserialize)]
struct ErrorDetail {
    code: Option<String>,
    message: Option<String>,
}

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", encode(key), encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

async fn stripe_request(
    http: &reqwest::Client,
    config: &StripeConfig,
    method: reqwest::Method,
    path: &str,
    body: Option<&str>,
    idempotency_key: Option<&str>,
) -> Result<Value, StripeError> {
    let url = format!("{}{path}", config.api_base);
    let mut request = http
        .request(method, url)
        .bearer_auth(&config.secret_key)
        .header("Stripe-Version", API_VERSION);
    if let Some(body) = body {
        request = request
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body.to_string());
    }
    if let Some(key) = idempotency_key {
        request = request.header("Idempotency-Key", key);
    }
    let response = request.send().await.map_err(|err| {
        eprintln!("Stripe {path} request failed: {err}");
        StripeError::Unavailable
    })?;
    let status = response.status();
    let bytes = response.bytes().await.map_err(|err| {
        eprintln!("Stripe {path} response could not be read: {err}");
        StripeError::Unavailable
    })?;
    if !status.is_success() {
        let envelope: ErrorEnvelope = serde_json::from_slice(&bytes).unwrap_or_default();
        let code = envelope
            .error
            .and_then(|detail| detail.code.or(detail.message))
            .unwrap_or_else(|| format!("http_{}", status.as_u16()));
        eprintln!("Stripe {path} rejected the request: {code}");
        return Err(StripeError::Api(code));
    }
    serde_json::from_slice(&bytes).map_err(|err| {
        eprintln!("Stripe {path} returned unexpected JSON: {err}");
        StripeError::Unavailable
    })
}

pub async fn create_customer(
    http: &reqwest::Client,
    config: &StripeConfig,
    organization_id: &str,
    name: &str,
    email: Option<&str>,
    user_id: &str,
) -> Result<String, StripeError> {
    let mut pairs = vec![
        ("name", name),
        ("metadata[organization_id]", organization_id),
        ("metadata[workos_user_id]", user_id),
    ];
    if let Some(email) = email {
        pairs.push(("email", email));
    }
    let body = form(&pairs);
    let value = stripe_request(
        http,
        config,
        reqwest::Method::POST,
        "/v1/customers",
        Some(&body),
        Some(&format!("cadri-customer-{organization_id}")),
    )
    .await?;
    Ok(serde_json::from_value::<IdBody>(value)
        .map_err(|_| StripeError::Unavailable)?
        .id)
}

pub async fn create_trial_subscription(
    http: &reqwest::Client,
    config: &StripeConfig,
    organization_id: &str,
    customer_id: &str,
) -> Result<Subscription, StripeError> {
    let days = TRIAL_DAYS.to_string();
    let body = form(&[
        ("customer", customer_id),
        ("items[0][price]", config.price_id.as_str()),
        ("trial_period_days", days.as_str()),
        (
            "trial_settings[end_behavior][missing_payment_method]",
            "pause",
        ),
        ("metadata[organization_id]", organization_id),
    ]);
    let value = stripe_request(
        http,
        config,
        reqwest::Method::POST,
        "/v1/subscriptions",
        Some(&body),
        Some(&format!("cadri-subscription-{organization_id}")),
    )
    .await?;
    serde_json::from_value(value).map_err(|_| StripeError::Unavailable)
}

pub async fn get_subscription(
    http: &reqwest::Client,
    config: &StripeConfig,
    subscription_id: &str,
) -> Result<Subscription, StripeError> {
    let value = stripe_request(
        http,
        config,
        reqwest::Method::GET,
        &format!("/v1/subscriptions/{subscription_id}"),
        None,
        None,
    )
    .await?;
    serde_json::from_value(value).map_err(|_| StripeError::Unavailable)
}

pub async fn customer_has_payment_method(
    http: &reqwest::Client,
    config: &StripeConfig,
    customer_id: &str,
) -> Result<bool, StripeError> {
    let value = stripe_request(
        http,
        config,
        reqwest::Method::GET,
        &format!("/v1/customers/{customer_id}"),
        None,
        None,
    )
    .await?;
    let customer: Customer = serde_json::from_value(value).map_err(|_| StripeError::Unavailable)?;
    Ok(customer
        .invoice_settings
        .default_payment_method
        .is_some_and(|method| !method.is_empty()))
}

pub async fn resume_subscription(
    http: &reqwest::Client,
    config: &StripeConfig,
    subscription_id: &str,
) -> Result<Subscription, StripeError> {
    let value = stripe_request(
        http,
        config,
        reqwest::Method::POST,
        &format!("/v1/subscriptions/{subscription_id}/resume"),
        Some(""),
        None,
    )
    .await?;
    serde_json::from_value(value).map_err(|_| StripeError::Unavailable)
}

pub async fn cancel_subscription(
    http: &reqwest::Client,
    config: &StripeConfig,
    subscription_id: &str,
) -> Result<Subscription, StripeError> {
    let value = stripe_request(
        http,
        config,
        reqwest::Method::DELETE,
        &format!("/v1/subscriptions/{subscription_id}"),
        None,
        None,
    )
    .await?;
    serde_json::from_value(value).map_err(|_| StripeError::Unavailable)
}

/// Portal sessions use Cadri's configuration, not the Stripe account default.
/// Payment method updates are on. Cancellation and invoice history are off until chosen deliberately.
pub async fn portal_url(
    http: &reqwest::Client,
    config: &StripeConfig,
    customer_id: &str,
    return_url: &str,
) -> Result<String, StripeError> {
    let configuration = portal_configuration(http, config).await?;
    let body = form(&[
        ("customer", customer_id),
        ("return_url", return_url),
        ("configuration", configuration.as_str()),
    ]);
    let value = stripe_request(
        http,
        config,
        reqwest::Method::POST,
        "/v1/billing_portal/sessions",
        Some(&body),
        None,
    )
    .await?;
    Ok(serde_json::from_value::<PortalBody>(value)
        .map_err(|_| StripeError::Unavailable)?
        .url)
}

/// Reports the current seat count. Stripe's "last" aggregation only sees events inside the
/// billing period, and WorkOS sends `workos_seat_count` when membership changes, so the first
/// paid period can otherwise bill zero seats.
pub async fn report_seat_count(
    http: &reqwest::Client,
    config: &StripeConfig,
    customer_id: &str,
    seats: u64,
) -> Result<(), StripeError> {
    let seats = seats.to_string();
    let timestamp = unix_now().to_string();
    let body = form(&[
        ("event_name", "workos_seat_count"),
        ("payload[stripe_customer_id]", customer_id),
        ("payload[value]", seats.as_str()),
        ("timestamp", timestamp.as_str()),
    ]);
    stripe_request(
        http,
        config,
        reqwest::Method::POST,
        "/v1/billing/meter_events",
        Some(&body),
        None,
    )
    .await?;
    Ok(())
}

fn portal_features() -> String {
    form(&[
        ("features[payment_method_update][enabled]", "true"),
        ("features[invoice_history][enabled]", "false"),
        ("features[subscription_cancel][enabled]", "false"),
        ("features[subscription_update][enabled]", "false"),
        ("features[customer_update][enabled]", "false"),
        ("metadata[cadri]", "portal"),
    ])
}

async fn portal_configuration(
    http: &reqwest::Client,
    config: &StripeConfig,
) -> Result<String, StripeError> {
    let listed = stripe_request(
        http,
        config,
        reqwest::Method::GET,
        "/v1/billing_portal/configurations?limit=100",
        None,
        None,
    )
    .await?;
    if let Some(id) = find_portal_configuration(&listed) {
        stripe_request(
            http,
            config,
            reqwest::Method::POST,
            &format!("/v1/billing_portal/configurations/{id}"),
            Some(&portal_features()),
            None,
        )
        .await?;
        return Ok(id);
    }
    let created = stripe_request(
        http,
        config,
        reqwest::Method::POST,
        "/v1/billing_portal/configurations",
        Some(&portal_features()),
        Some("cadri-portal-configuration"),
    )
    .await?;
    created
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or(StripeError::Unavailable)
}

fn find_portal_configuration(page: &Value) -> Option<String> {
    page.get("data")?.as_array()?.iter().find_map(|item| {
        let marker = item
            .get("metadata")
            .and_then(|metadata| metadata.get("cadri"))
            .and_then(Value::as_str);
        if marker == Some("portal") {
            item.get("id").and_then(Value::as_str).map(str::to_string)
        } else {
            None
        }
    })
}
