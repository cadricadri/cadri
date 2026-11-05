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
    // Stripe rejects pause-at-trial-end on subscriptions that include metered prices.
    // Cadri's seat price is metered, so missing-card trials must cancel; a later card
    // starts a new paid subscription (see create_paid_subscription).
    let body = form(&[
        ("customer", customer_id),
        ("items[0][price]", config.price_id.as_str()),
        ("trial_period_days", days.as_str()),
        (
            "trial_settings[end_behavior][missing_payment_method]",
            "cancel",
        ),
        ("metadata[organization_id]", organization_id),
    ]);
    let value = stripe_request(
        http,
        config,
        reqwest::Method::POST,
        "/v1/subscriptions",
        Some(&body),
        // v2: prior pause+metered creates failed and Stripe cached that idempotency result.
        Some(&format!("cadri-subscription-v2-{organization_id}")),
    )
    .await?;
    serde_json::from_value(value).map_err(|_| StripeError::Unavailable)
}

/// Starts a paid subscription after a canceled trial once the customer has a card.
pub async fn create_paid_subscription(
    http: &reqwest::Client,
    config: &StripeConfig,
    organization_id: &str,
    customer_id: &str,
) -> Result<Subscription, StripeError> {
    let body = form(&[
        ("customer", customer_id),
        ("items[0][price]", config.price_id.as_str()),
        ("metadata[organization_id]", organization_id),
    ]);
    let value = stripe_request(
        http,
        config,
        reqwest::Method::POST,
        "/v1/subscriptions",
        Some(&body),
        Some(&format!("cadri-paid-{organization_id}")),
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

/// How many finalized invoices the billing page asks Stripe for. One page, not the full history.
pub const INVOICE_LIST_LIMIT: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BillingInvoice {
    pub id: String,
    /// `status_transitions.paid_at` when the invoice is paid, otherwise `created`.
    pub date: i64,
    pub description: String,
    /// `amount_paid` when paid, otherwise `amount_due`, in the invoice currency.
    pub amount: String,
    pub currency: String,
    /// Hosted invoice page, or a charge receipt when Stripe did not send one.
    pub receipt_url: Option<String>,
}

/// Latest finalized invoices for this Stripe customer. Drafts are left out.
/// A missing customer is an empty page, not a Stripe call.
pub async fn list_invoices(
    http: &reqwest::Client,
    config: &StripeConfig,
    customer_id: &str,
) -> Result<Vec<BillingInvoice>, StripeError> {
    if customer_id.is_empty() {
        return Ok(Vec::new());
    }
    let path = format!(
        "/v1/invoices?customer={}&limit={INVOICE_LIST_LIMIT}",
        encode(customer_id),
    );
    let page = stripe_request(http, config, reqwest::Method::GET, &path, None, None).await?;
    Ok(invoices_from_list(&page, customer_id))
}

fn invoices_from_list(page: &Value, customer_id: &str) -> Vec<BillingInvoice> {
    let Some(data) = page.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    for invoice in data {
        if let Some(row) = invoice_row(invoice, customer_id) {
            rows.push(row);
        }
    }
    rows.sort_by(|left, right| right.date.cmp(&left.date).then(right.id.cmp(&left.id)));
    rows.truncate(INVOICE_LIST_LIMIT);
    rows
}

fn invoice_row(invoice: &Value, customer_id: &str) -> Option<BillingInvoice> {
    let id = invoice_id(invoice)?;
    if !is_finalized(invoice) || !belongs_to_customer(invoice, customer_id) {
        return None;
    }
    let date = invoice_date(invoice)?;
    let currency = invoice_currency(invoice);
    Some(BillingInvoice {
        id: id.to_string(),
        date,
        description: invoice_description(invoice),
        amount: format_amount(invoice_amount(invoice), &currency),
        currency,
        receipt_url: receipt_link(invoice),
    })
}

fn invoice_id(invoice: &Value) -> Option<&str> {
    if invoice.get("object").and_then(Value::as_str) != Some("invoice") {
        return None;
    }
    let id = text(invoice.get("id"))?;
    // List results are real invoices. Preview rows use a different id prefix.
    if id.starts_with("in_") {
        Some(id)
    } else {
        None
    }
}

fn is_finalized(invoice: &Value) -> bool {
    matches!(
        invoice.get("status").and_then(Value::as_str),
        Some("open" | "paid" | "uncollectible" | "void")
    )
}

fn belongs_to_customer(invoice: &Value, customer_id: &str) -> bool {
    match invoice.get("customer") {
        Some(Value::String(id)) => id == customer_id,
        Some(customer) => customer.get("id").and_then(Value::as_str) == Some(customer_id),
        None => false,
    }
}

/// Paid rows use `paid_at`. Anything still unpaid uses `created`.
fn invoice_date(invoice: &Value) -> Option<i64> {
    let created = unix_stamp(invoice.get("created"));
    if invoice.get("status").and_then(Value::as_str) == Some("paid") {
        unix_stamp(
            invoice
                .get("status_transitions")
                .and_then(|transitions| transitions.get("paid_at")),
        )
        .or(created)
    } else {
        created
    }
}

fn unix_stamp(value: Option<&Value>) -> Option<i64> {
    value.and_then(Value::as_i64).filter(|stamp| *stamp > 0)
}

fn invoice_amount(invoice: &Value) -> i64 {
    let field = if invoice.get("status").and_then(Value::as_str) == Some("paid") {
        "amount_paid"
    } else {
        "amount_due"
    };
    invoice.get(field).and_then(Value::as_i64).unwrap_or(0)
}

fn invoice_currency(invoice: &Value) -> String {
    text(invoice.get("currency"))
        .unwrap_or("usd")
        .to_ascii_lowercase()
}

fn invoice_description(invoice: &Value) -> String {
    if let Some(description) = text(invoice.get("description")) {
        return clip(description);
    }
    if let Some(summary) = line_summary(invoice) {
        return clip(&summary);
    }
    if let Some(number) = text(invoice.get("number")) {
        return clip(&format!("Invoice {number}"));
    }
    "Invoice".to_string()
}

fn line_summary(invoice: &Value) -> Option<String> {
    let lines = invoice
        .get("lines")
        .and_then(|lines| lines.get("data"))
        .and_then(Value::as_array)?;
    let mut parts = Vec::new();
    for line in lines {
        let Some(description) = text(line.get("description")) else {
            continue;
        };
        if parts
            .iter()
            .any(|existing: &String| existing == description)
        {
            continue;
        }
        parts.push(description.to_string());
        if parts.len() == 3 {
            break;
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

const DESCRIPTION_MAX: usize = 160;

fn clip(text: &str) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(DESCRIPTION_MAX).collect();
    if chars.next().is_some() {
        let mut shorter: String = head.chars().take(DESCRIPTION_MAX - 1).collect();
        shorter.push('…');
        shorter
    } else {
        head
    }
}

fn text(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

/// Prefer the hosted invoice. A charge `receipt_url` is only used when that is missing.
fn receipt_link(invoice: &Value) -> Option<String> {
    stripe_page_url(invoice.get("hosted_invoice_url")).or_else(|| charge_receipt_url(invoice))
}

fn charge_receipt_url(invoice: &Value) -> Option<String> {
    if let Some(url) = stripe_page_url(
        invoice
            .get("charge")
            .and_then(|charge| charge.get("receipt_url")),
    ) {
        return Some(url);
    }
    let payments = invoice
        .get("payments")
        .and_then(|payments| payments.get("data"))
        .and_then(Value::as_array)?;
    for payment in payments {
        if let Some(url) = stripe_page_url(
            payment
                .pointer("/payment/charge")
                .and_then(|charge| charge.get("receipt_url")),
        ) {
            return Some(url);
        }
        if let Some(url) = stripe_page_url(
            payment
                .pointer("/payment/payment_intent/latest_charge")
                .and_then(|charge| charge.get("receipt_url")),
        ) {
            return Some(url);
        }
    }
    None
}

fn stripe_page_url(value: Option<&Value>) -> Option<String> {
    let url = text(value)?;
    let parsed = reqwest::Url::parse(url).ok()?;
    if parsed.scheme() != "https" || !parsed.username().is_empty() || parsed.password().is_some() {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    if host == "stripe.com" || host.ends_with(".stripe.com") {
        Some(url.to_string())
    } else {
        None
    }
}

/// Stripe amounts are minor units. Zero-decimal currencies are already major units.
/// ISK and UGX stay two-decimal for Stripe's compatibility amounts. HUF and TWD do too.
fn format_amount(amount: i64, currency: &str) -> String {
    let code = currency.trim().to_ascii_lowercase();
    let scale = minor_unit_scale(&code);
    let negative = amount < 0;
    let amount = amount.unsigned_abs();
    let factor = 10u64.pow(scale);
    let (major, minor) = if scale == 0 {
        (amount, 0)
    } else {
        (amount / factor, amount % factor)
    };
    let major_text = group_thousands(major);
    let number = if scale == 0 {
        major_text
    } else {
        format!("{major_text}.{minor:0width$}", width = scale as usize)
    };
    let body = match code.as_str() {
        "usd" => format!("${number}"),
        "eur" => format!("€{number}"),
        "gbp" => format!("£{number}"),
        "jpy" => format!("¥{number}"),
        _ => format!("{number} {}", code.to_ascii_uppercase()),
    };
    if negative {
        format!("-{body}")
    } else {
        body
    }
}

fn minor_unit_scale(currency: &str) -> u32 {
    match currency {
        "bif" | "clp" | "djf" | "gnf" | "jpy" | "kmf" | "krw" | "mga" | "pyg" | "rwf" | "vnd"
        | "vuv" | "xaf" | "xof" | "xpf" => 0,
        "bhd" | "jod" | "kwd" | "omr" | "tnd" => 3,
        _ => 2,
    }
}

fn group_thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn page(data: Vec<Value>) -> Value {
        json!({ "object": "list", "data": data })
    }

    #[test]
    fn finalized_invoices_use_paid_at_amount_paid_and_the_hosted_invoice() {
        let rows = invoices_from_list(
            &page(vec![json!({
                "id": "in_paid",
                "object": "invoice",
                "customer": "cus_1",
                "status": "paid",
                "created": 1_700_000_000,
                "number": "INV-0001",
                "description": null,
                "amount_paid": 2000,
                "amount_due": 2000,
                "currency": "usd",
                "hosted_invoice_url": "https://invoice.stripe.com/i/acct_test/paid",
                "status_transitions": { "paid_at": 1_700_000_100 },
                "lines": { "data": [{ "description": "Cadri" }] }
            })]),
            "cus_1",
        );
        assert_eq!(
            rows,
            vec![BillingInvoice {
                id: "in_paid".to_string(),
                date: 1_700_000_100,
                description: "Cadri".to_string(),
                amount: "$20.00".to_string(),
                currency: "usd".to_string(),
                receipt_url: Some("https://invoice.stripe.com/i/acct_test/paid".to_string()),
            }]
        );
    }

    #[test]
    fn unpaid_invoices_use_created_amount_due_and_a_charge_receipt() {
        let rows = invoices_from_list(
            &page(vec![json!({
                "id": "in_open",
                "object": "invoice",
                "customer": { "id": "cus_1", "object": "customer" },
                "status": "open",
                "created": 1_700_000_200,
                "description": "  ",
                "number": "INV-0002",
                "amount_paid": 0,
                "amount_due": 4000,
                "currency": "USD",
                "hosted_invoice_url": null,
                "status_transitions": { "paid_at": null },
                "lines": { "data": [{ "description": null }] },
                "payments": {
                    "data": [{
                        "payment": {
                            "charge": {
                                "receipt_url": "https://pay.stripe.com/receipts/invoices/test"
                            }
                        }
                    }]
                }
            })]),
            "cus_1",
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].date, 1_700_000_200);
        assert_eq!(rows[0].amount, "$40.00");
        assert_eq!(rows[0].description, "Invoice INV-0002");
        assert_eq!(
            rows[0].receipt_url.as_deref(),
            Some("https://pay.stripe.com/receipts/invoices/test")
        );
    }

    #[test]
    fn drafts_other_customers_and_non_stripe_receipts_are_dropped() {
        let rows = invoices_from_list(
            &page(vec![
                json!({
                    "id": "in_draft",
                    "object": "invoice",
                    "customer": "cus_1",
                    "status": "draft",
                    "created": 1_700_000_300,
                    "amount_due": 100,
                    "currency": "usd"
                }),
                json!({
                    "id": "in_other",
                    "object": "invoice",
                    "customer": "cus_other",
                    "status": "paid",
                    "created": 1_700_000_400,
                    "amount_paid": 900,
                    "currency": "usd",
                    "hosted_invoice_url": "https://invoice.stripe.com/i/acct_test/other",
                    "status_transitions": { "paid_at": 1_700_000_400 }
                }),
                json!({
                    "id": "upcoming_in_1",
                    "object": "invoice",
                    "customer": "cus_1",
                    "status": "open",
                    "created": 1_700_000_500,
                    "amount_due": 100,
                    "currency": "usd"
                }),
                json!({
                    "id": "in_void",
                    "object": "invoice",
                    "customer": "cus_1",
                    "status": "void",
                    "created": 1_700_000_050,
                    "description": "Seat adjustment",
                    "amount_paid": 2000,
                    "amount_due": 0,
                    "currency": "usd",
                    "hosted_invoice_url": "http://invoice.stripe.com/not-https",
                    "charge": { "receipt_url": "https://evil.example/receipt" }
                }),
            ]),
            "cus_1",
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "in_void");
        assert_eq!(rows[0].date, 1_700_000_050);
        assert_eq!(rows[0].amount, "$0.00");
        assert_eq!(rows[0].description, "Seat adjustment");
        assert_eq!(rows[0].receipt_url, None);
    }

    #[test]
    fn amounts_follow_stripe_minor_units() {
        assert_eq!(format_amount(2000, "usd"), "$20.00");
        assert_eq!(format_amount(0, "usd"), "$0.00");
        assert_eq!(format_amount(100_000, "usd"), "$1,000.00");
        assert_eq!(format_amount(-250, "usd"), "-$2.50");
        assert_eq!(format_amount(500, "jpy"), "¥500");
        assert_eq!(format_amount(5_120, "kwd"), "5.120 KWD");
        assert_eq!(format_amount(500, "ugx"), "5.00 UGX");
        assert_eq!(format_amount(500, "isk"), "5.00 ISK");
    }

    #[test]
    fn an_empty_invoice_page_stays_empty() {
        assert!(invoices_from_list(&json!({ "data": [] }), "cus_1").is_empty());
        assert!(invoices_from_list(&json!({}), "cus_1").is_empty());
    }
}
