use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{Path, State};
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

#[derive(Clone)]
struct Mock {
    base: String,
    refreshes: Arc<AtomicUsize>,
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
    let tokens = |refresh_token: &str| {
        Json(json!({
            "user": { "id": "user_1", "email": "user1@example.com" },
            "organization_id": "org_1",
            "access_token": mock.valid_token("org_1"),
            "refresh_token": refresh_token,
        }))
        .into_response()
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
                tokens("refresh-from-code")
            } else {
                error("invalid_grant")
            }
        }
        "refresh_token" => {
            assert_eq!(body["client_secret"], API_KEY);
            mock.refreshes.fetch_add(1, Ordering::SeqCst);
            if body["refresh_token"] == "good-refresh" {
                tokens("rotated-refresh")
            } else {
                error("invalid_grant")
            }
        }
        "urn:ietf:params:oauth:grant-type:device_code" => {
            match body["device_code"].as_str().unwrap() {
                "approved" => tokens("device-refresh"),
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

async fn mock_organization(Path(id): Path<String>, headers: HeaderMap) -> Response {
    if !has_api_key(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let name = if id == "org_1" { "Org One" } else { "Org Two" };
    Json(json!({ "object": "organization", "id": id, "name": name })).into_response()
}

async fn setup() -> (Router, Mock) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let mock = Mock {
        base: base.clone(),
        refreshes: Arc::new(AtomicUsize::new(0)),
    };
    let mock_app = Router::new()
        .route("/user_management/authenticate", post(mock_authenticate))
        .route("/user_management/authorize/device", post(mock_device_start))
        .route("/sso/jwks/{client_id}", get(mock_jwks))
        .route("/user_management/users/{id}", get(mock_user))
        .route("/organizations/{id}", get(mock_organization))
        .with_state(mock.clone());
    tokio::spawn(async move { axum::serve(listener, mock_app).await.unwrap() });

    let workos = Arc::new(
        WorkOs::new(Config {
            api_key: API_KEY.to_string(),
            client_id: CLIENT_ID.to_string(),
            cookie_key: cookie_key(),
            public_url: PUBLIC_URL.to_string(),
            api_base: base,
        })
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
