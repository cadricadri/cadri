use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::cookie::{Cookie, Key, PrivateCookieJar};
use cadri_ee::{Config, WorkOs};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{json, Value};
use tower::ServiceExt;

const CLIENT_ID: &str = "client_test";
const API_KEY: &str = "sk_test_key";
const PUBLIC_URL: &str = "https://cadri.test";
const STRIPE_KEY: &str = "sk_test_stripe";
const STRIPE_PRICE: &str = "price_test";
const STRIPE_WEBHOOK: &str = "whsec_test";
const SIGNING_KEY: &[u8] = include_bytes!("fixtures/test-only-signing-key.pem");
const OTHER_KEY: &[u8] = include_bytes!("fixtures/test-only-other-key.pem");
const JWKS: &str = include_str!("fixtures/test-only-jwks.json");

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn cookie_key() -> Key {
    Key::from(&[7u8; 64])
}

fn sign(key: &[u8], claims: Value) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("test-key".to_string());
    jsonwebtoken::encode(&header, &claims, &EncodingKey::from_rsa_pem(key).unwrap()).unwrap()
}

#[derive(Default)]
struct Directory {
    memberships: Vec<(String, String, String)>,
    organizations: Vec<(String, String)>,
    invitations: Vec<Value>,
    admin_roles: Vec<(String, String)>,
    /// WorkOS `list_metadata.after`. A non-empty cursor means more memberships exist.
    membership_after: Option<String>,
    /// WorkOS organization id -> Stripe customer id, set by the billing update.
    stripe_customers: Vec<(String, String)>,
    subscription_status: String,
    trial_end: Option<i64>,
    payment_method: Option<String>,
    meter_events: Vec<String>,
    resumes: usize,
    portal_sessions: usize,
    cancels: usize,
    portal_configuration: Option<String>,
}

#[derive(Clone)]
struct Mock {
    base: String,
    refreshes: Arc<AtomicUsize>,
    directory: Arc<Mutex<Directory>>,
}

impl Mock {
    fn issuer(&self) -> String {
        format!("{}/user_management/{CLIENT_ID}", self.base)
    }

    fn token(&self, org_id: Option<&str>, exp: i64) -> String {
        let mut claims = json!({
            "iss": self.issuer(),
            "sub": "user_1",
            "sid": "session_1",
            "exp": exp,
            "iat": now(),
        });
        if let Some(org_id) = org_id {
            claims["org_id"] = json!(org_id);
        }
        sign(SIGNING_KEY, claims)
    }

    fn valid_token(&self, org_id: &str) -> String {
        self.token(Some(org_id), now() + 300)
    }
}

async fn mock_authenticate(State(mock): State<Mock>, Json(body): Json<Value>) -> Response {
    let issue = |refresh_token: &str, org_id: Option<&str>| {
        let mut payload = json!({
            "user": { "id": "user_1", "email": "user1@example.com" },
            "access_token": mock.token(org_id, now() + 300),
            "refresh_token": refresh_token,
        });
        if let Some(org_id) = org_id {
            payload["organization_id"] = json!(org_id);
        }
        Json(payload).into_response()
    };
    let error = |code: &str| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": code, "error_description": "test" })),
        )
            .into_response()
    };
    assert_eq!(body["client_id"], CLIENT_ID);
    match body["grant_type"].as_str().unwrap() {
        "authorization_code" => {
            assert_eq!(body["client_secret"], API_KEY);
            if body["code"] == "good-code" {
                issue("refresh-from-code", Some("org_1"))
            } else {
                error("invalid_grant")
            }
        }
        "refresh_token" => {
            assert_eq!(body["client_secret"], API_KEY);
            mock.refreshes.fetch_add(1, Ordering::SeqCst);
            let refresh = body["refresh_token"].as_str().unwrap_or("");
            let requested = body["organization_id"]
                .as_str()
                .filter(|org_id| !org_id.is_empty())
                .map(str::to_string);
            match (requested, refresh) {
                (Some(org_id), "good-refresh" | "no-org-refresh") => {
                    issue("rotated-refresh", Some(&org_id))
                }
                (None, "good-refresh") => issue("rotated-refresh", Some("org_1")),
                (None, "no-org-refresh") => issue("rotated-no-org", None),
                _ => error("invalid_grant"),
            }
        }
        "urn:ietf:params:oauth:grant-type:device_code" => {
            match body["device_code"].as_str().unwrap() {
                "approved" => issue("device-refresh", Some("org_1")),
                pending_or_error => error(pending_or_error),
            }
        }
        other => panic!("unexpected grant_type {other}"),
    }
}

async fn mock_device_start(Json(body): Json<Value>) -> Json<Value> {
    assert_eq!(body["client_id"], CLIENT_ID);
    Json(json!({
        "device_code": "device-code-1",
        "user_code": "BCDF-GHJK",
        "verification_uri": "https://auth.test/device",
        "verification_uri_complete": "https://auth.test/device?user_code=BCDF-GHJK",
        "expires_in": 300,
        "interval": 5,
    }))
}

async fn mock_jwks(Path(client_id): Path<String>) -> Response {
    if client_id != CLIENT_ID {
        return StatusCode::NOT_FOUND.into_response();
    }
    ([(header::CONTENT_TYPE, "application/json")], JWKS).into_response()
}

fn has_api_key(headers: &HeaderMap) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        == Some(&format!("Bearer {API_KEY}"))
}

async fn mock_user(Path(id): Path<String>, headers: HeaderMap) -> Response {
    if !has_api_key(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(json!({ "object": "user", "id": id, "email": "user1@example.com" })).into_response()
}

async fn mock_organization(
    State(mock): State<Mock>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !has_api_key(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let directory = mock.directory.lock().unwrap();
    let name = directory
        .organizations
        .iter()
        .find(|(org_id, _)| org_id == &id)
        .map(|(_, name)| name.clone())
        .unwrap_or_else(|| {
            if id == "org_1" {
                "Org One".to_string()
            } else {
                "Org Two".to_string()
            }
        });
    Json(json!({ "object": "organization", "id": id, "name": name })).into_response()
}

fn api_error(code: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "code": code, "message": "test" })),
    )
        .into_response()
}

async fn mock_list_memberships(
    State(mock): State<Mock>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if !has_api_key(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let user_id = query.get("user_id").cloned().unwrap_or_default();
    let organization_id = query.get("organization_id").cloned().unwrap_or_default();
    let directory = mock.directory.lock().unwrap();
    let data: Vec<Value> = directory
        .memberships
        .iter()
        .filter(|(user, org, _)| {
            (user_id.is_empty() || user == &user_id)
                && (organization_id.is_empty() || org == &organization_id)
        })
        .map(|(user, org_id, status)| {
            json!({
                "object": "organization_membership",
                "user_id": user,
                "organization_id": org_id,
                "status": status,
            })
        })
        .collect();
    // The "another page" flag is for user lookups. Seat counts list by organization and
    // must not follow a cursor that never ends.
    let after = if user_id.is_empty() {
        None
    } else {
        directory.membership_after.clone()
    };
    Json(json!({ "data": data, "list_metadata": { "before": null, "after": after } }))
        .into_response()
}

async fn mock_create_organization(
    State(mock): State<Mock>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !has_api_key(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let name = body["name"].as_str().unwrap_or("").to_string();
    let mut directory = mock.directory.lock().unwrap();
    let id = format!("org_new_{}", directory.organizations.len() + 1);
    directory.organizations.push((id.clone(), name.clone()));
    Json(json!({ "object": "organization", "id": id, "name": name })).into_response()
}

async fn mock_create_membership(
    State(mock): State<Mock>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !has_api_key(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let user_id = body["user_id"].as_str().unwrap_or("").to_string();
    let organization_id = body["organization_id"].as_str().unwrap_or("").to_string();
    let role = body["role_slug"].as_str().unwrap_or("").to_string();
    let mut directory = mock.directory.lock().unwrap();
    directory.admin_roles.push((user_id.clone(), role));
    directory
        .memberships
        .push((user_id, organization_id.clone(), "active".to_string()));
    Json(json!({
        "object": "organization_membership",
        "user_id": body["user_id"],
        "organization_id": organization_id,
        "status": "active",
    }))
    .into_response()
}

async fn mock_create_invitation(
    State(mock): State<Mock>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !has_api_key(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let email = body["email"].as_str().unwrap_or("");
    if email == "member@example.com" {
        return api_error("user_already_organization_member");
    }
    if email == "pending@example.com" {
        return api_error("invitation_already_exists");
    }
    mock.directory
        .lock()
        .unwrap()
        .invitations
        .push(body.clone());
    Json(json!({
        "object": "invitation",
        "id": "invitation_1",
        "email": email,
        "state": "pending",
        "organization_id": body["organization_id"],
    }))
    .into_response()
}

fn stripe_authorized(headers: &HeaderMap) -> bool {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        == Some(&format!("Bearer {STRIPE_KEY}"));
    let version = headers
        .get("stripe-version")
        .and_then(|value| value.to_str().ok());
    bearer && version == Some(cadri_ee::API_VERSION)
}

fn subscription_json(directory: &Directory) -> Value {
    let status = if directory.subscription_status.is_empty() {
        "trialing"
    } else {
        directory.subscription_status.as_str()
    };
    json!({
        "id": "sub_1",
        "object": "subscription",
        "customer": "cus_1",
        "status": status,
        "trial_end": directory.trial_end.unwrap_or(now() + 30 * 24 * 60 * 60),
        "default_payment_method": directory.payment_method,
    })
}

async fn mock_update_organization(
    State(mock): State<Mock>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !has_api_key(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let customer = body["stripe_customer_id"]
        .as_str()
        .unwrap_or("")
        .to_string();
    mock.directory
        .lock()
        .unwrap()
        .stripe_customers
        .push((id.clone(), customer.clone()));
    Json(json!({ "id": id, "name": body["name"], "stripe_customer_id": customer })).into_response()
}

async fn mock_stripe_customer(headers: HeaderMap, body: String) -> Response {
    if !stripe_authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    assert!(
        body.contains("organization_id"),
        "stripe customer is not tied to a team: {body}"
    );
    Json(json!({ "id": "cus_1" })).into_response()
}

async fn mock_stripe_get_customer(headers: HeaderMap, State(mock): State<Mock>) -> Response {
    if !stripe_authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let method = mock.directory.lock().unwrap().payment_method.clone();
    Json(json!({
        "id": "cus_1",
        "invoice_settings": { "default_payment_method": method }
    }))
    .into_response()
}

async fn mock_stripe_subscription(
    headers: HeaderMap,
    State(mock): State<Mock>,
    body: String,
) -> Response {
    if !stripe_authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    assert!(body.contains("trial_period_days=30"), "{body}");
    assert!(
        body.contains("missing_payment_method=pause") || body.contains("pause"),
        "{body}"
    );
    let mut directory = mock.directory.lock().unwrap();
    if directory.trial_end.is_none() {
        directory.trial_end = Some(now() + 30 * 24 * 60 * 60);
    }
    Json(subscription_json(&directory)).into_response()
}

async fn mock_stripe_get_subscription(headers: HeaderMap, State(mock): State<Mock>) -> Response {
    if !stripe_authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let directory = mock.directory.lock().unwrap();
    Json(subscription_json(&directory)).into_response()
}

async fn mock_stripe_resume(headers: HeaderMap, State(mock): State<Mock>) -> Response {
    if !stripe_authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut directory = mock.directory.lock().unwrap();
    directory.resumes += 1;
    directory.subscription_status = "active".to_string();
    Json(subscription_json(&directory)).into_response()
}

async fn mock_stripe_cancel(headers: HeaderMap, State(mock): State<Mock>) -> Response {
    if !stripe_authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut directory = mock.directory.lock().unwrap();
    directory.cancels += 1;
    directory.subscription_status = "canceled".to_string();
    Json(subscription_json(&directory)).into_response()
}

async fn mock_list_portal_configurations(headers: HeaderMap, State(mock): State<Mock>) -> Response {
    if !stripe_authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let id = mock.directory.lock().unwrap().portal_configuration.clone();
    let data = id
        .map(|id| json!([{ "id": id, "metadata": { "cadri": "portal" } }]))
        .unwrap_or_else(|| json!([]));
    Json(json!({ "data": data })).into_response()
}

async fn mock_save_portal_configuration(
    headers: HeaderMap,
    State(mock): State<Mock>,
    body: String,
) -> Response {
    if !stripe_authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    assert!(
        body.contains("features%5Bpayment_method_update%5D%5Benabled%5D=true"),
        "{body}"
    );
    assert!(
        body.contains("features%5Binvoice_history%5D%5Benabled%5D=false"),
        "{body}"
    );
    assert!(
        body.contains("features%5Bsubscription_cancel%5D%5Benabled%5D=false"),
        "{body}"
    );
    let mut directory = mock.directory.lock().unwrap();
    if directory.portal_configuration.is_none() {
        directory.portal_configuration = Some("bpc_1".to_string());
    }
    let id = directory.portal_configuration.clone().unwrap_or_default();
    Json(json!({ "id": id, "metadata": { "cadri": "portal" } })).into_response()
}

async fn mock_stripe_portal(
    headers: HeaderMap,
    State(mock): State<Mock>,
    body: String,
) -> Response {
    if !stripe_authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    assert!(body.contains("bpc_1"), "{body}");
    mock.directory.lock().unwrap().portal_sessions += 1;
    Json(json!({ "url": "https://billing.stripe.test/p/session_test" })).into_response()
}

async fn mock_stripe_meter(headers: HeaderMap, State(mock): State<Mock>, body: String) -> Response {
    if !stripe_authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    mock.directory.lock().unwrap().meter_events.push(body);
    Json(json!({ "object": "billing.meter_event" })).into_response()
}

async fn setup() -> (Router, Mock) {
    setup_inner(false).await
}

async fn setup_billing() -> (Router, Mock) {
    setup_inner(true).await
}

async fn setup_inner(billing: bool) -> (Router, Mock) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let mock = Mock {
        base: base.clone(),
        refreshes: Arc::new(AtomicUsize::new(0)),
        directory: Arc::new(Mutex::new(Directory::default())),
    };
    let mock_app = Router::new()
        .route("/user_management/authenticate", post(mock_authenticate))
        .route("/user_management/authorize/device", post(mock_device_start))
        .route(
            "/user_management/organization_memberships",
            get(mock_list_memberships).post(mock_create_membership),
        )
        .route("/user_management/invitations", post(mock_create_invitation))
        .route("/sso/jwks/{client_id}", get(mock_jwks))
        .route("/user_management/users/{id}", get(mock_user))
        .route("/organizations", post(mock_create_organization))
        .route(
            "/organizations/{id}",
            get(mock_organization).put(mock_update_organization),
        )
        .route("/v1/customers", post(mock_stripe_customer))
        .route("/v1/customers/{id}", get(mock_stripe_get_customer))
        .route("/v1/subscriptions", post(mock_stripe_subscription))
        .route(
            "/v1/subscriptions/{id}",
            get(mock_stripe_get_subscription).delete(mock_stripe_cancel),
        )
        .route("/v1/subscriptions/{id}/resume", post(mock_stripe_resume))
        .route(
            "/v1/billing_portal/configurations",
            get(mock_list_portal_configurations).post(mock_save_portal_configuration),
        )
        .route(
            "/v1/billing_portal/configurations/{id}",
            post(mock_save_portal_configuration),
        )
        .route("/v1/billing_portal/sessions", post(mock_stripe_portal))
        .route("/v1/billing/meter_events", post(mock_stripe_meter))
        .with_state(mock.clone());
    tokio::spawn(async move { axum::serve(listener, mock_app).await.unwrap() });

    let workos = Arc::new(
        WorkOs::with_stripe(
            Config {
                api_key: API_KEY.to_string(),
                client_id: CLIENT_ID.to_string(),
                cookie_key: cookie_key(),
                public_url: PUBLIC_URL.to_string(),
                api_base: base.clone(),
            },
            billing.then(|| cadri_ee::StripeConfig {
                secret_key: STRIPE_KEY.to_string(),
                price_id: STRIPE_PRICE.to_string(),
                webhook_secret: STRIPE_WEBHOOK.to_string(),
                api_base: base,
            }),
        )
        .unwrap(),
    );
    let engine = cadri_core::router(
        cadri_core::db::open_in_memory().unwrap(),
        workos.clone(),
        Some(cadri_ee::router(workos)),
    );
    (engine, mock)
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, headers, body)
}

fn request(
    method: Method,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    match body {
        Some(body) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

fn get_with(uri: &str, headers: &[(&str, &str)]) -> Request<Body> {
    request(Method::GET, uri, headers, None)
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// The full `Set-Cookie` header for `name`, if any.
fn set_cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with(&format!("{name}=")))
}

/// `name=value` from a `Set-Cookie` header, ready for a `Cookie` request header.
fn cookie_pair(set_cookie: &str) -> String {
    set_cookie.split(';').next().unwrap().to_string()
}

fn encrypted_session(access_token: &str, refresh_token: &str) -> String {
    let value = json!({ "access_token": access_token, "refresh_token": refresh_token }).to_string();
    let response = PrivateCookieJar::new(cookie_key())
        .add(Cookie::new(cadri_ee::SESSION_COOKIE, value))
        .into_response();
    cookie_pair(set_cookie(response.headers(), cadri_ee::SESSION_COOKIE).unwrap())
}

fn query_param(url: &str, name: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

#[tokio::test]
async fn auth_config_reports_workos() {
    let (app, _) = setup().await;
    let (status, _, body) = send(&app, get_with("/api/auth/config", &[])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "mode": "workos" }));
}

#[tokio::test]
async fn bearer_token_is_accepted() {
    let (app, mock) = setup().await;
    let auth = bearer(&mock.valid_token("org_1"));
    let (status, _, body) = send(&app, get_with("/api/me", &[("authorization", &auth)])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({
            "user_id": "user_1",
            "email": "user1@example.com",
            "org_id": "org_1",
            "org_name": "Org One",
        })
    );
}

#[tokio::test]
async fn bearer_token_with_the_plain_api_issuer_is_accepted() {
    let (app, mock) = setup().await;
    let token = sign(
        SIGNING_KEY,
        json!({ "iss": mock.base, "sub": "user_1", "org_id": "org_1", "exp": now() + 300 }),
    );
    let (status, _, _) = send(
        &app,
        get_with("/api/me", &[("authorization", &bearer(&token))]),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn invalid_bearer_tokens_are_rejected() {
    let (app, mock) = setup().await;
    let claims =
        |iss: &str, exp: i64| json!({ "iss": iss, "sub": "user_1", "org_id": "org_1", "exp": exp });
    let cases = [
        (
            "wrong signature",
            sign(OTHER_KEY, claims(&mock.issuer(), now() + 300)),
        ),
        ("expired", mock.token(Some("org_1"), now() - 3600)),
        (
            "wrong issuer",
            sign(SIGNING_KEY, claims("https://evil.test", now() + 300)),
        ),
        ("garbage", "not-a-jwt".to_string()),
    ];
    for (name, token) in cases {
        let (status, _, body) = send(
            &app,
            get_with("/api/transcripts", &[("authorization", &bearer(&token))]),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{name}");
        assert_eq!(body, json!({ "error": "unauthorized" }), "{name}");
    }
    let (status, _, _) = send(&app, get_with("/api/transcripts", &[])).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn token_without_org_is_forbidden() {
    let (app, mock) = setup().await;
    let token = mock.token(None, now() + 300);
    let (status, _, body) = send(
        &app,
        get_with("/api/me", &[("authorization", &bearer(&token))]),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body, json!({ "error": "no_organization" }));
}

#[tokio::test]
async fn workos_orgs_are_isolated() {
    let (app, mock) = setup().await;
    let org_1 = bearer(&mock.valid_token("org_1"));
    let org_2 = bearer(&mock.valid_token("org_2"));
    let (status, _, _) = send(
        &app,
        request(
            Method::POST,
            "/api/transcripts",
            &[("authorization", &org_1)],
            Some(json!({ "session_id": "s1", "content": "secret" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, _, listed) = send(
        &app,
        get_with("/api/transcripts", &[("authorization", &org_1)]),
    )
    .await;
    assert_eq!(listed.as_array().unwrap().len(), 1);
    let (_, _, listed) = send(
        &app,
        get_with("/api/transcripts", &[("authorization", &org_2)]),
    )
    .await;
    assert_eq!(listed, json!([]));
}

#[tokio::test]
async fn device_start_proxies_workos() {
    let (app, _) = setup().await;
    let (status, _, body) = send(
        &app,
        request(Method::POST, "/auth/device/start", &[], Some(json!({}))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["device_code"], "device-code-1");
    assert_eq!(body["user_code"], "BCDF-GHJK");
    assert_eq!(body["verification_uri"], "https://auth.test/device");
    assert_eq!(
        body["verification_uri_complete"],
        "https://auth.test/device?user_code=BCDF-GHJK"
    );
    assert_eq!(body["expires_in"], 300);
    assert_eq!(body["interval"], 5);
}

#[tokio::test]
async fn device_token_maps_workos_errors() {
    let (app, _) = setup().await;
    for (code, status) in [
        ("authorization_pending", StatusCode::PRECONDITION_REQUIRED),
        ("slow_down", StatusCode::TOO_MANY_REQUESTS),
        ("expired_token", StatusCode::BAD_REQUEST),
        ("access_denied", StatusCode::BAD_REQUEST),
    ] {
        let (actual, _, body) = send(
            &app,
            request(
                Method::POST,
                "/auth/device/token",
                &[],
                Some(json!({ "device_code": code })),
            ),
        )
        .await;
        assert_eq!(actual, status, "{code}");
        assert_eq!(body, json!({ "error": code }));
    }
}

#[tokio::test]
async fn device_token_returns_tokens_once_approved() {
    let (app, _) = setup().await;
    let (status, _, body) = send(
        &app,
        request(
            Method::POST,
            "/auth/device/token",
            &[],
            Some(json!({ "device_code": "approved" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["refresh_token"], "device-refresh");
    let expires_at = body["expires_at"].as_i64().unwrap();
    assert!((now() + 290..=now() + 310).contains(&expires_at));

    let auth = bearer(body["access_token"].as_str().unwrap());
    let (status, _, _) = send(&app, get_with("/api/me", &[("authorization", &auth)])).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn refresh_endpoint_rotates_tokens() {
    let (app, _) = setup().await;
    let (status, _, body) = send(
        &app,
        request(
            Method::POST,
            "/auth/refresh",
            &[],
            Some(json!({ "refresh_token": "good-refresh" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["refresh_token"], "rotated-refresh");
    assert!(body["access_token"].is_string());
    assert!(body["expires_at"].is_i64());

    let (status, _, body) = send(
        &app,
        request(
            Method::POST,
            "/auth/refresh",
            &[],
            Some(json!({ "refresh_token": "revoked" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body, json!({ "error": "unauthorized" }));
}

#[tokio::test]
async fn login_redirects_to_authkit() {
    let (app, mock) = setup().await;
    let (status, headers, _) =
        send(&app, get_with("/auth/login?return_to=/transcripts", &[])).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let location = headers.get(header::LOCATION).unwrap().to_str().unwrap();
    assert!(location.starts_with(&format!("{}/user_management/authorize?", mock.base)));
    assert_eq!(
        query_param(location, "provider").as_deref(),
        Some("authkit")
    );
    assert_eq!(
        query_param(location, "client_id").as_deref(),
        Some(CLIENT_ID)
    );
    assert_eq!(
        query_param(location, "redirect_uri").as_deref(),
        Some("https://cadri.test/auth/callback")
    );
    assert_eq!(
        query_param(location, "response_type").as_deref(),
        Some("code")
    );
    assert!(query_param(location, "state").is_some());

    let login_cookie = set_cookie(&headers, cadri_ee::LOGIN_COOKIE).unwrap();
    for attribute in ["HttpOnly", "Secure", "SameSite=Lax", "Path=/auth"] {
        assert!(login_cookie.contains(attribute), "{login_cookie}");
    }
}

#[tokio::test]
async fn login_rejects_unsafe_return_to() {
    let (app, _) = setup().await;
    for return_to in [
        "%2F%2Fevil.com",
        "https%3A%2F%2Fevil.com",
        "%2F%5Cevil.com",
        "evil",
    ] {
        let (status, headers, body) = send(
            &app,
            get_with(&format!("/auth/login?return_to={return_to}"), &[]),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{return_to}");
        assert_eq!(body, json!({ "error": "invalid_return_to" }));
        assert!(headers.get(header::LOCATION).is_none());
    }
    let (status, headers, _) = send(&app, get_with("/auth/login", &[])).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(headers.get(header::LOCATION).is_some());
}

async fn start_login(app: &Router, return_to: &str) -> (String, String) {
    let (_, headers, _) = send(
        app,
        get_with(&format!("/auth/login?return_to={return_to}"), &[]),
    )
    .await;
    let location = headers.get(header::LOCATION).unwrap().to_str().unwrap();
    let state = query_param(location, "state").unwrap();
    let cookie = cookie_pair(set_cookie(&headers, cadri_ee::LOGIN_COOKIE).unwrap());
    (state, cookie)
}

#[tokio::test]
async fn callback_sets_the_session_cookie() {
    let (app, _) = setup().await;
    let (state, login_cookie) = start_login(&app, "/transcripts").await;
    let uri = format!("/auth/callback?code=good-code&state={state}");
    let (status, headers, _) = send(&app, get_with(&uri, &[("cookie", &login_cookie)])).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers.get(header::LOCATION).unwrap(), "/transcripts");

    let session = set_cookie(&headers, cadri_ee::SESSION_COOKIE).unwrap();
    for attribute in ["HttpOnly", "Secure", "SameSite=Lax", "Path=/"] {
        assert!(session.contains(attribute), "{session}");
    }
    assert!(!session.contains("good-refresh") && !session.contains("refresh-from-code"));
    let cleared = set_cookie(&headers, cadri_ee::LOGIN_COOKIE).unwrap();
    assert!(cleared.contains("Max-Age=0"), "{cleared}");

    let (status, headers, body) = send(
        &app,
        get_with("/api/me", &[("cookie", &cookie_pair(session))]),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["org_id"], "org_1");
    assert!(set_cookie(&headers, cadri_ee::SESSION_COOKIE).is_none());
}

#[tokio::test]
async fn callback_rejects_bad_state() {
    let (app, _) = setup().await;
    let (state, _) = start_login(&app, "/").await;
    let (_, other_cookie) = start_login(&app, "/").await;
    let uri = format!("/auth/callback?code=good-code&state={state}");
    for cookies in [vec![("cookie", other_cookie.as_str())], vec![]] {
        let (status, headers, body) = send(&app, get_with(&uri, &cookies)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body, json!({ "error": "invalid_state" }));
        assert!(set_cookie(&headers, cadri_ee::SESSION_COOKIE).is_none());
    }
}

#[tokio::test]
async fn callback_rejects_bad_code() {
    let (app, _) = setup().await;
    let (state, login_cookie) = start_login(&app, "/").await;
    let uri = format!("/auth/callback?code=bad-code&state={state}");
    let (status, headers, _) = send(&app, get_with(&uri, &[("cookie", &login_cookie)])).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(set_cookie(&headers, cadri_ee::SESSION_COOKIE).is_none());
}

#[tokio::test]
async fn expired_session_cookie_is_refreshed_once() {
    let (app, mock) = setup().await;
    let cookie = encrypted_session(&mock.token(Some("org_1"), now() - 3600), "good-refresh");
    let first = send(&app, get_with("/api/me", &[("cookie", &cookie)]));
    let second = send(&app, get_with("/api/transcripts", &[("cookie", &cookie)]));
    let ((status_1, headers_1, body_1), (status_2, headers_2, _)) = tokio::join!(first, second);
    assert_eq!(status_1, StatusCode::OK);
    assert_eq!(status_2, StatusCode::OK);
    assert_eq!(body_1["user_id"], "user_1");
    assert_eq!(mock.refreshes.load(Ordering::SeqCst), 1);

    let refreshed = set_cookie(&headers_1, cadri_ee::SESSION_COOKIE).unwrap();
    assert!(refreshed.contains("HttpOnly") && refreshed.contains("Secure"));
    assert!(set_cookie(&headers_2, cadri_ee::SESSION_COOKIE).is_some());

    let (status, headers, _) = send(
        &app,
        get_with("/api/me", &[("cookie", &cookie_pair(refreshed))]),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(set_cookie(&headers, cadri_ee::SESSION_COOKIE).is_none());
}

#[tokio::test]
async fn session_cookie_with_revoked_refresh_token_is_unauthorized() {
    let (app, mock) = setup().await;
    let cookie = encrypted_session(&mock.token(Some("org_1"), now() - 3600), "revoked");
    let (status, _, body) = send(&app, get_with("/api/me", &[("cookie", &cookie)])).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body, json!({ "error": "unauthorized" }));
}

#[tokio::test]
async fn tampered_session_cookie_is_unauthorized() {
    let (app, _) = setup().await;
    let (status, _, _) = send(
        &app,
        get_with("/api/me", &[("cookie", "cadri_session=not-encrypted")]),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn logout_clears_the_cookie_and_returns_the_workos_url() {
    let (app, mock) = setup().await;
    let cookie = encrypted_session(&mock.token(Some("org_1"), now() - 3600), "good-refresh");
    let (status, headers, body) = send(
        &app,
        request(Method::POST, "/auth/logout", &[("cookie", &cookie)], None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({
            "logout_url": format!("{}/user_management/sessions/logout?session_id=session_1", mock.base)
        })
    );
    let cleared = set_cookie(&headers, cadri_ee::SESSION_COOKIE).unwrap();
    assert!(
        cleared.contains("Max-Age=0") && cleared.contains("Path=/"),
        "{cleared}"
    );

    let (status, _, body) = send(&app, request(Method::POST, "/auth/logout", &[], None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "logout_url": "/" }));
}

fn no_org_cookie(mock: &Mock) -> String {
    encrypted_session(&mock.token(None, now() + 300), "no-org-refresh")
}

fn seed_membership(mock: &Mock, org_id: &str) {
    seed_membership_with_status(mock, org_id, "active");
}

fn seed_membership_with_status(mock: &Mock, org_id: &str, status: &str) {
    mock.directory.lock().unwrap().memberships.push((
        "user_1".to_string(),
        org_id.to_string(),
        status.to_string(),
    ));
}

#[tokio::test]
async fn naming_a_team_creates_an_organization_and_selects_it() {
    let (app, mock) = setup().await;
    let cookie = no_org_cookie(&mock);
    let (status, headers, body) = send(
        &app,
        request(
            Method::POST,
            "/auth/organization",
            &[("cookie", &cookie)],
            Some(json!({ "name": "  Company A  " })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["org_id"], "org_new_1");
    assert_eq!(body["org_name"], "Company A");
    assert_eq!(body["user_id"], "user_1");
    assert_eq!(body["email"], "user1@example.com");
    let (organizations, admin_roles) = {
        let directory = mock.directory.lock().unwrap();
        (
            directory.organizations.clone(),
            directory.admin_roles.clone(),
        )
    };
    assert_eq!(
        organizations,
        vec![("org_new_1".to_string(), "Company A".to_string())]
    );
    assert_eq!(
        admin_roles,
        vec![("user_1".to_string(), "admin".to_string())]
    );
    assert_eq!(mock.refreshes.load(Ordering::SeqCst), 1);

    let session = cookie_pair(set_cookie(&headers, cadri_ee::SESSION_COOKIE).unwrap());
    let (status, _, me) = send(&app, get_with("/api/me", &[("cookie", &session)])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["org_id"], "org_new_1");
}

#[tokio::test]
async fn an_existing_membership_is_selected_instead_of_creating_another_team() {
    let (app, mock) = setup().await;
    seed_membership(&mock, "org_1");
    let cookie = no_org_cookie(&mock);
    let (status, _, body) = send(
        &app,
        request(
            Method::POST,
            "/auth/organization",
            &[("cookie", &cookie)],
            Some(json!({ "name": "Other Name" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["org_id"], "org_1");
    assert_eq!(body["org_name"], "Org One");
    assert!(mock.directory.lock().unwrap().organizations.is_empty());
}

#[tokio::test]
async fn an_inactive_membership_does_not_count_as_a_team() {
    let (app, mock) = setup().await;
    seed_membership_with_status(&mock, "org_old", "inactive");
    let cookie = no_org_cookie(&mock);
    let (status, _, body) = send(
        &app,
        request(
            Method::POST,
            "/auth/organization",
            &[("cookie", &cookie)],
            Some(json!({ "name": "Company A" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["org_id"], "org_new_1");
    assert_eq!(body["org_name"], "Company A");
}

#[tokio::test]
async fn another_page_of_memberships_is_not_a_team_switcher() {
    let (app, mock) = setup().await;
    seed_membership(&mock, "org_1");
    mock.directory.lock().unwrap().membership_after = Some("next-page".to_string());
    let cookie = no_org_cookie(&mock);
    let (status, _, body) = send(
        &app,
        request(
            Method::POST,
            "/auth/organization",
            &[("cookie", &cookie)],
            Some(json!({ "name": "Company A" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body, json!({ "error": "multiple_organizations" }));
    assert!(mock.directory.lock().unwrap().organizations.is_empty());
}

#[tokio::test]
async fn several_memberships_are_not_a_team_switcher() {
    let (app, mock) = setup().await;
    seed_membership(&mock, "org_1");
    seed_membership(&mock, "org_2");
    let cookie = no_org_cookie(&mock);
    let (status, headers, body) = send(
        &app,
        request(
            Method::POST,
            "/auth/organization",
            &[("cookie", &cookie)],
            Some(json!({ "name": "Company A" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body, json!({ "error": "multiple_organizations" }));
    assert!(set_cookie(&headers, cadri_ee::SESSION_COOKIE).is_none());
    assert!(mock.directory.lock().unwrap().organizations.is_empty());
    assert_eq!(mock.refreshes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_session_that_already_has_an_organization_does_not_create_another() {
    let (app, mock) = setup().await;
    let cookie = encrypted_session(&mock.valid_token("org_1"), "good-refresh");
    let (status, headers, body) = send(
        &app,
        request(
            Method::POST,
            "/auth/organization",
            &[("cookie", &cookie)],
            Some(json!({ "name": "Company B" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body, json!({ "error": "already_in_organization" }));
    assert!(set_cookie(&headers, cadri_ee::SESSION_COOKIE).is_none());
    assert!(mock.directory.lock().unwrap().organizations.is_empty());
    assert_eq!(mock.refreshes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_expired_session_keeps_its_organization() {
    let (app, mock) = setup().await;
    let cookie = encrypted_session(&mock.token(Some("org_1"), now() - 3600), "good-refresh");
    let (status, headers, body) = send(
        &app,
        request(
            Method::POST,
            "/auth/organization",
            &[("cookie", &cookie)],
            Some(json!({ "name": "Company B" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["org_id"], "org_1");
    assert!(set_cookie(&headers, cadri_ee::SESSION_COOKIE).is_some());
    assert!(mock.directory.lock().unwrap().organizations.is_empty());
}

#[tokio::test]
async fn creating_a_team_requires_a_session_and_a_name() {
    let (app, _) = setup().await;
    let (status, _, body) = send(
        &app,
        request(
            Method::POST,
            "/auth/organization",
            &[],
            Some(json!({ "name": "Company A" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body, json!({ "error": "unauthorized" }));

    let (app, mock) = setup().await;
    let cookie = no_org_cookie(&mock);
    let (status, _, body) = send(
        &app,
        request(
            Method::POST,
            "/auth/organization",
            &[("cookie", &cookie)],
            Some(json!({ "name": "   " })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, json!({ "error": "invalid_organization_name" }));
    assert_eq!(mock.refreshes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_member_invites_by_email_into_the_session_organization() {
    let (app, mock) = setup().await;
    let cookie = encrypted_session(&mock.valid_token("org_1"), "good-refresh");
    let (status, _, body) = send(
        &app,
        request(
            Method::POST,
            "/auth/invitations",
            &[("cookie", &cookie)],
            Some(json!({ "email": " Dev2@CompanyA.example ", "organization_id": "org_evil" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({ "email": "Dev2@CompanyA.example", "state": "pending" })
    );
    let invitations = mock.directory.lock().unwrap().invitations.clone();
    assert_eq!(invitations.len(), 1);
    assert_eq!(invitations[0]["email"], "Dev2@CompanyA.example");
    assert_eq!(invitations[0]["organization_id"], "org_1");
    assert_eq!(invitations[0]["inviter_user_id"], "user_1");
    assert!(invitations[0].get("organization_id") != Some(&json!("org_evil")));
}

#[tokio::test]
async fn invitations_report_an_existing_member_or_invite() {
    let (app, mock) = setup().await;
    let cookie = encrypted_session(&mock.valid_token("org_1"), "good-refresh");
    for (email, error) in [
        ("member@example.com", "already_member"),
        ("pending@example.com", "already_invited"),
    ] {
        let (status, _, body) = send(
            &app,
            request(
                Method::POST,
                "/auth/invitations",
                &[("cookie", &cookie)],
                Some(json!({ "email": email })),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{email}");
        assert_eq!(body, json!({ "error": error }));
    }
    assert!(mock.directory.lock().unwrap().invitations.is_empty());
}

#[tokio::test]
async fn an_invitation_requires_an_organization() {
    let (app, mock) = setup().await;
    let cookie = no_org_cookie(&mock);
    let (status, _, body) = send(
        &app,
        request(
            Method::POST,
            "/auth/invitations",
            &[("cookie", &cookie)],
            Some(json!({ "email": "dev2@companya.example" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body, json!({ "error": "no_organization" }));

    let (status, _, body) = send(
        &app,
        request(
            Method::POST,
            "/auth/invitations",
            &[],
            Some(json!({ "email": "dev2@companya.example" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body, json!({ "error": "unauthorized" }));

    let cookie = encrypted_session(&mock.valid_token("org_1"), "good-refresh");
    let (status, _, body) = send(
        &app,
        request(
            Method::POST,
            "/auth/invitations",
            &[("cookie", &cookie)],
            Some(json!({ "email": "not-an-email" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, json!({ "error": "invalid_email" }));
    assert!(mock.directory.lock().unwrap().invitations.is_empty());
}

#[test]
fn cookie_key_must_be_long_base64() {
    assert!(cadri_ee::cookie_key_from_base64("not base64!").is_err());
    assert!(cadri_ee::cookie_key_from_base64(&"QUFB".repeat(10)).is_err());
    assert!(cadri_ee::cookie_key_from_base64(&"QUFB".repeat(22)).is_ok());
}

#[test]
fn binary_fails_fast_without_required_env() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cadri-engine-ee"))
        .env_clear()
        .env("CADRI_DB", "/nonexistent/should-not-be-created.db")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    for name in [
        "WORKOS_API_KEY",
        "WORKOS_CLIENT_ID",
        "CADRI_COOKIE_KEY",
        "CADRI_PUBLIC_URL",
    ] {
        assert!(stderr.contains(name), "{stderr}");
    }
}

/// Creates a real WorkOS user with no organization, then drives the Cadri engine against
/// `https://api.workos.com`. Run with `--ignored` and `WORKOS_API_KEY` / `WORKOS_CLIENT_ID`
/// from the staging environment. The user and organization are deleted before the test returns.
#[tokio::test]
#[ignore = "creates a staging WorkOS user; set WORKOS_API_KEY and WORKOS_CLIENT_ID"]
async fn staging_user_without_an_organization_can_name_a_team() {
    if let Err(err) = staging_signup().await {
        panic!("{err}");
    }
}

async fn staging_signup() -> Result<(), String> {
    let api_key =
        std::env::var("WORKOS_API_KEY").map_err(|_| "WORKOS_API_KEY is not set".to_string())?;
    let client_id =
        std::env::var("WORKOS_CLIENT_ID").map_err(|_| "WORKOS_CLIENT_ID is not set".to_string())?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| err.to_string())?
        .as_nanos();
    let email = format!("cadri-signup-{stamp}@cadri.io");
    let password = format!("Cadri-Signup-{stamp}aA!");
    let team = format!("Signup Check {stamp}");
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|err| err.to_string())?;

    let mut created = StagingCreated::default();
    let result = staging_signup_attempt(
        &http,
        &StagingAccount {
            api_key: api_key.clone(),
            client_id,
            email,
            password,
            team,
        },
        &mut created,
    )
    .await;
    let cleaned = staging_cleanup(
        &http,
        &api_key,
        created.user_id.as_deref(),
        created.org_id.as_deref(),
    )
    .await;
    match (result, cleaned) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(err), Ok(())) => Err(err),
        (Ok(()), Err(err)) => Err(format!("cleanup failed: {err}")),
        (Err(err), Err(cleanup)) => Err(format!("{err}; cleanup failed: {cleanup}")),
    }
}

struct StagingAccount {
    api_key: String,
    client_id: String,
    email: String,
    password: String,
    team: String,
}

#[derive(Default)]
struct StagingCreated {
    user_id: Option<String>,
    org_id: Option<String>,
}

async fn staging_signup_attempt(
    http: &reqwest::Client,
    account: &StagingAccount,
    created: &mut StagingCreated,
) -> Result<(), String> {
    let created_user = workos_call(
        http.post("https://api.workos.com/user_management/users")
            .bearer_auth(&account.api_key)
            .json(&json!({
                "email": account.email,
                "password": account.password,
                "email_verified": true,
                "first_name": "Signup",
                "last_name": "Check",
            })),
    )
    .await?;
    created.user_id = created_user
        .get("id")
        .and_then(|id| id.as_str())
        .map(str::to_string);
    if created.user_id.is_none() {
        return Err("WorkOS create user returned no id".to_string());
    }

    let session = workos_call(
        http.post("https://api.workos.com/user_management/authenticate")
            .json(&json!({
                "client_id": account.client_id,
                "client_secret": account.api_key,
                "grant_type": "password",
                "email": account.email,
                "password": account.password,
            })),
    )
    .await?;
    let access_token = session
        .get("access_token")
        .and_then(|token| token.as_str())
        .ok_or("WorkOS password grant returned no access_token")?;
    let refresh_token = session
        .get("refresh_token")
        .and_then(|token| token.as_str())
        .ok_or("WorkOS password grant returned no refresh_token")?;
    let cookie = encrypted_session(access_token, refresh_token);

    let workos = Arc::new(WorkOs::new(Config {
        api_key: account.api_key.clone(),
        client_id: account.client_id.clone(),
        cookie_key: cookie_key(),
        public_url: PUBLIC_URL.to_string(),
        api_base: "https://api.workos.com".to_string(),
    })?);
    let app = cadri_core::router(
        cadri_core::db::open_in_memory().map_err(|err| err.to_string())?,
        workos.clone(),
        Some(cadri_ee::router(workos)),
    );

    let (status, headers, body) = send(
        &app,
        request(
            Method::POST,
            "/auth/organization",
            &[("cookie", &cookie)],
            Some(json!({ "name": account.team })),
        ),
    )
    .await;
    if status == StatusCode::CONFLICT && body["error"] == "already_in_organization" {
        return Err(
            "a brand-new user already belonged to an organization; domain auto-join may be on"
                .to_string(),
        );
    }
    if status != StatusCode::OK {
        return Err(format!(
            "POST /auth/organization returned {status} {}",
            body["error"].as_str().unwrap_or("unknown")
        ));
    }
    let created_org = body["org_id"]
        .as_str()
        .ok_or("organization response has no org_id")?
        .to_string();
    if body["org_name"] != account.team || body["email"] != account.email {
        return Err("organization response did not echo the team name and email".to_string());
    }
    created.org_id = Some(created_org.clone());

    let session_cookie = set_cookie(&headers, cadri_ee::SESSION_COOKIE)
        .map(cookie_pair)
        .ok_or("naming a team did not set a session cookie")?;
    let (status, _, me) = send(&app, get_with("/api/me", &[("cookie", &session_cookie)])).await;
    if status != StatusCode::OK || me["org_id"] != created_org {
        return Err(format!(
            "GET /api/me returned {status} for org {}",
            me["error"].as_str().unwrap_or("unknown")
        ));
    }

    let transcript_id = format!("signup-{}", created_org.trim_start_matches("org_"));
    let (status, _, stored) = send(
        &app,
        request(
            Method::POST,
            "/api/transcripts",
            &[("cookie", &session_cookie)],
            Some(json!({
                "session_id": transcript_id,
                "content": "signup check",
                "cwd": "/signup",
                "hook_event": "SignupCheck",
            })),
        ),
    )
    .await;
    if status != StatusCode::OK || stored["session_id"] != transcript_id {
        return Err(format!(
            "POST /api/transcripts returned {status} {}",
            stored["error"].as_str().unwrap_or("unknown")
        ));
    }

    let (status, _, invite) = send(
        &app,
        request(
            Method::POST,
            "/auth/invitations",
            &[("cookie", &session_cookie)],
            Some(json!({ "email": account.email, "organization_id": "org_evil" })),
        ),
    )
    .await;
    if status != StatusCode::CONFLICT || invite["error"] != "already_member" {
        return Err(format!(
            "inviting the founder returned {status} {}",
            invite["error"].as_str().unwrap_or("unknown")
        ));
    }
    Ok(())
}

async fn workos_call(request: reqwest::RequestBuilder) -> Result<Value, String> {
    let response = request.send().await.map_err(|err| err.to_string())?;
    let status = response.status();
    let bytes = response.bytes().await.map_err(|err| err.to_string())?;
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    if !status.is_success() {
        let code = body
            .get("code")
            .or_else(|| body.get("error"))
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        return Err(format!("WorkOS HTTP {} {code}", status.as_u16()));
    }
    Ok(body)
}

async fn staging_cleanup(
    http: &reqwest::Client,
    api_key: &str,
    user_id: Option<&str>,
    org_id: Option<&str>,
) -> Result<(), String> {
    if let Some(user_id) = user_id {
        workos_call(
            http.delete(format!(
                "https://api.workos.com/user_management/users/{user_id}"
            ))
            .bearer_auth(api_key),
        )
        .await?;
    }
    if let Some(org_id) = org_id {
        workos_call(
            http.delete(format!("https://api.workos.com/organizations/{org_id}"))
                .bearer_auth(api_key),
        )
        .await?;
    }
    Ok(())
}

async fn open_team(app: &Router, mock: &Mock) -> (String, String) {
    let cookie = no_org_cookie(mock);
    let (status, headers, body) = send(
        app,
        request(
            Method::POST,
            "/auth/organization",
            &[("cookie", &cookie)],
            Some(json!({ "name": "Company A" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let session = cookie_pair(set_cookie(&headers, cadri_ee::SESSION_COOKIE).unwrap());
    let org_id = body["org_id"].as_str().unwrap().to_string();
    (session, org_id)
}

fn entitled_token(mock: &Mock, org_id: &str) -> String {
    sign(
        SIGNING_KEY,
        json!({
            "iss": mock.issuer(),
            "sub": "user_1",
            "sid": "session_1",
            "org_id": org_id,
            "exp": now() + 300,
            "iat": now(),
            "entitlements": ["cadri"],
        }),
    )
}

#[tokio::test]
async fn billing_stays_off_until_stripe_is_configured() {
    let (app, mock) = setup().await;
    let auth = bearer(&mock.valid_token("org_1"));
    let (status, _, body) = send(&app, get_with("/api/billing", &[("authorization", &auth)])).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, json!({ "error": "billing_disabled" }));
    let (status, _, _) = send(
        &app,
        get_with("/api/transcripts", &[("authorization", &auth)]),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn naming_a_team_starts_a_thirty_day_trial() {
    let (app, mock) = setup_billing().await;
    let (session, org_id) = open_team(&app, &mock).await;
    assert_eq!(org_id, "org_new_1");
    assert_eq!(
        mock.directory.lock().unwrap().stripe_customers,
        vec![("org_new_1".to_string(), "cus_1".to_string())]
    );
    let (status, _, billing) = send(&app, get_with("/api/billing", &[("cookie", &session)])).await;
    assert_eq!(status, StatusCode::OK, "{billing}");
    assert_eq!(billing["status"], "trialing");
    assert_eq!(billing["access"], true);
    assert_eq!(billing["seat_count"], 1);
    assert!(billing["trial_end"].as_i64().unwrap() > now());
    let (status, _, _) = send(&app, get_with("/api/transcripts", &[("cookie", &session)])).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_paused_trial_locks_the_product_until_the_entitlement_returns() {
    let (app, mock) = setup_billing().await;
    let (session, org_id) = open_team(&app, &mock).await;
    let payload = json!({
        "id": "evt_pause",
        "type": "customer.subscription.paused",
        "data": {
            "object": {
                "id": "sub_1",
                "customer": "cus_1",
                "status": "paused",
                "trial_end": now() - 60,
            }
        }
    });
    let raw = payload.to_string();
    let signature = cadri_ee::sign_webhook(STRIPE_WEBHOOK, now(), raw.as_bytes());
    let (status, _, body) = send(
        &app,
        request(
            Method::POST,
            "/api/stripe/webhook",
            &[("stripe-signature", &signature)],
            Some(payload),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, _, body) = send(&app, get_with("/api/transcripts", &[("cookie", &session)])).await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
    assert_eq!(body, json!({ "error": "payment_required" }));
    let (status, _, _) = send(&app, get_with("/api/me", &[("cookie", &session)])).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, body) = send(
        &app,
        request(
            Method::POST,
            "/auth/invitations",
            &[("cookie", &session)],
            Some(json!({ "email": "new@company.example" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
    assert_eq!(body, json!({ "error": "payment_required" }));

    let auth = bearer(&entitled_token(&mock, &org_id));
    let (status, _, _) = send(
        &app,
        get_with("/api/transcripts", &[("authorization", &auth)]),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn an_active_subscription_reports_the_current_seat_count() {
    let (app, mock) = setup_billing().await;
    let (_session, _) = open_team(&app, &mock).await;
    let payload = json!({
        "id": "evt_active",
        "type": "customer.subscription.updated",
        "data": {
            "object": {
                "id": "sub_1",
                "customer": "cus_1",
                "status": "active",
                "trial_end": now() - 60,
            }
        }
    });
    let raw = payload.to_string();
    let signature = cadri_ee::sign_webhook(STRIPE_WEBHOOK, now(), raw.as_bytes());
    let (status, _, _) = send(
        &app,
        request(
            Method::POST,
            "/api/stripe/webhook",
            &[("stripe-signature", &signature)],
            Some(payload),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let events = mock.directory.lock().unwrap().meter_events.clone();
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(events[0].contains("workos_seat_count"), "{}", events[0]);
    assert!(events[0].contains("cus_1"), "{}", events[0]);
    assert!(events[0].contains("=1"), "{}", events[0]);
}

#[tokio::test]
async fn a_bad_webhook_signature_is_rejected() {
    let (app, _) = setup_billing().await;
    let payload = json!({ "id": "evt", "type": "customer.subscription.updated" });
    let (status, _, body) = send(
        &app,
        request(
            Method::POST,
            "/api/stripe/webhook",
            &[("stripe-signature", "t=1,v1=00")],
            Some(payload),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, json!({ "error": "invalid_signature" }));
}

#[tokio::test]
async fn the_billing_portal_resumes_a_paused_trial_that_has_a_card() {
    let (app, mock) = setup_billing().await;
    let (session, _) = open_team(&app, &mock).await;
    {
        let mut directory = mock.directory.lock().unwrap();
        directory.subscription_status = "paused".to_string();
        directory.payment_method = Some("pm_1".to_string());
    }
    let (status, _, body) = send(
        &app,
        request(
            Method::POST,
            "/api/billing/portal",
            &[("cookie", &session)],
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["url"], "https://billing.stripe.test/p/session_test");

    let (status, _, billing) = send(
        &app,
        get_with("/api/billing?portal_return=1", &[("cookie", &session)]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{billing}");
    assert_eq!(billing["status"], "active");
    assert_eq!(mock.directory.lock().unwrap().resumes, 1);
}

async fn post_webhook(app: &Router, payload: Value) -> StatusCode {
    let raw = payload.to_string();
    let signature = cadri_ee::sign_webhook(STRIPE_WEBHOOK, now(), raw.as_bytes());
    let (status, _, _) = send(
        app,
        request(
            Method::POST,
            "/api/stripe/webhook",
            &[("stripe-signature", &signature)],
            Some(payload),
        ),
    )
    .await;
    status
}

#[tokio::test]
async fn a_repeated_active_event_reports_the_seat_count_once() {
    let (app, mock) = setup_billing().await;
    let (_session, _) = open_team(&app, &mock).await;
    let payload = json!({
        "id": "evt_active_once",
        "type": "customer.subscription.updated",
        "created": now(),
        "api_version": cadri_ee::API_VERSION,
        "data": {
            "object": {
                "id": "sub_1",
                "customer": "cus_1",
                "status": "active",
                "trial_end": now() - 60,
            }
        }
    });
    assert_eq!(post_webhook(&app, payload.clone()).await, StatusCode::OK);
    assert_eq!(post_webhook(&app, payload).await, StatusCode::OK);
    assert_eq!(mock.directory.lock().unwrap().meter_events.len(), 1);
}

#[tokio::test]
async fn an_older_event_does_not_replace_a_newer_subscription() {
    let (app, mock) = setup_billing().await;
    let (_session, org_id) = open_team(&app, &mock).await;
    let active = json!({
        "id": "evt_newer",
        "type": "customer.subscription.updated",
        "created": now(),
        "api_version": cadri_ee::API_VERSION,
        "data": {
            "object": {
                "id": "sub_1",
                "customer": "cus_1",
                "status": "active",
                "trial_end": now() - 60,
            }
        }
    });
    let stale = json!({
        "id": "evt_older",
        "type": "customer.subscription.updated",
        "created": now() - 1_000,
        "api_version": cadri_ee::API_VERSION,
        "data": {
            "object": {
                "id": "sub_1",
                "customer": "cus_1",
                "status": "past_due",
                "trial_end": now() - 60,
            }
        }
    });
    assert_eq!(post_webhook(&app, active).await, StatusCode::OK);
    assert_eq!(post_webhook(&app, stale).await, StatusCode::OK);
    let auth = bearer(&entitled_token(&mock, &org_id));
    let (status, _, _) = send(
        &app,
        get_with("/api/transcripts", &[("authorization", &auth)]),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_past_due_subscription_stays_locked_while_the_entitlement_remains() {
    let (app, mock) = setup_billing().await;
    let (_session, org_id) = open_team(&app, &mock).await;
    let payload = json!({
        "id": "evt_past_due",
        "type": "customer.subscription.updated",
        "created": now(),
        "api_version": cadri_ee::API_VERSION,
        "data": {
            "object": {
                "id": "sub_1",
                "customer": "cus_1",
                "status": "past_due",
                "trial_end": now() - 60,
            }
        }
    });
    assert_eq!(post_webhook(&app, payload).await, StatusCode::OK);
    let auth = bearer(&entitled_token(&mock, &org_id));
    let (status, _, body) = send(
        &app,
        get_with("/api/transcripts", &[("authorization", &auth)]),
    )
    .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
    assert_eq!(body, json!({ "error": "payment_required" }));
}

#[tokio::test]
async fn a_failed_invoice_with_no_retries_left_cancels_the_subscription() {
    let (app, mock) = setup_billing().await;
    let (_session, org_id) = open_team(&app, &mock).await;
    let payload = json!({
        "id": "evt_invoice_final",
        "type": "invoice.payment_failed",
        "created": now(),
        "api_version": cadri_ee::API_VERSION,
        "data": {
            "object": {
                "customer": "cus_1",
                "subscription": "sub_1",
                "next_payment_attempt": null,
            }
        }
    });
    assert_eq!(post_webhook(&app, payload.clone()).await, StatusCode::OK);
    assert_eq!(post_webhook(&app, payload).await, StatusCode::OK);
    assert_eq!(mock.directory.lock().unwrap().cancels, 1);
    let auth = bearer(&entitled_token(&mock, &org_id));
    let (status, _, _) = send(
        &app,
        get_with("/api/transcripts", &[("authorization", &auth)]),
    )
    .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
}

#[tokio::test]
async fn a_failed_invoice_keeps_retrying_until_stripe_stops() {
    let (app, mock) = setup_billing().await;
    let (_session, _) = open_team(&app, &mock).await;
    let payload = json!({
        "id": "evt_invoice_retry",
        "type": "invoice.payment_failed",
        "created": now(),
        "api_version": cadri_ee::API_VERSION,
        "data": {
            "object": {
                "customer": "cus_1",
                "subscription": "sub_1",
                "next_payment_attempt": 1_800_000_000,
            }
        }
    });
    assert_eq!(post_webhook(&app, payload).await, StatusCode::OK);
    assert_eq!(mock.directory.lock().unwrap().cancels, 0);
}

#[tokio::test]
async fn a_webhook_from_another_api_version_is_rejected() {
    let (app, mock) = setup_billing().await;
    let (_session, _) = open_team(&app, &mock).await;
    let payload = json!({
        "id": "evt_old_version",
        "type": "customer.subscription.updated",
        "created": now(),
        "api_version": "2020-08-27",
        "data": {
            "object": {
                "id": "sub_1",
                "customer": "cus_1",
                "status": "active",
            }
        }
    });
    assert_eq!(post_webhook(&app, payload).await, StatusCode::BAD_REQUEST);
    assert!(mock.directory.lock().unwrap().meter_events.is_empty());
}
