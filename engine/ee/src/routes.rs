use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::cookie::{Cookie, SameSite};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use cadri_core::ApiError;
use serde::{Deserialize, Serialize};
use serde_json::json;
use subtle::ConstantTimeEq;

use crate::workos::{
    read_session, session_cookie, workos_unavailable, DeviceAuthorization, Session, WorkOs,
    WorkOsError, SESSION_COOKIE,
};

pub const LOGIN_COOKIE: &str = "cadri_login";
const LOGIN_COOKIE_PATH: &str = "/auth";
const LOGIN_MAX_AGE_MINUTES: i64 = 10;

/// `/auth/*` routes for the WorkOS edition.
pub fn router(workos: Arc<WorkOs>) -> Router {
    Router::new()
        .route("/auth/login", get(login))
        .route("/auth/callback", get(callback))
        .route("/auth/logout", post(logout))
        .route("/auth/organization", post(create_organization))
        .route("/auth/invitations", post(create_invitation))
        .route("/auth/device/start", post(device_start))
        .route("/auth/device/token", post(device_token))
        .route("/auth/refresh", post(refresh))
        .route("/api/billing", get(billing_status))
        .route("/api/billing/portal", post(billing_portal))
        .route("/api/stripe/webhook", post(stripe_webhook))
        .with_state(workos)
}

/// A same-origin path: one leading '/', no scheme-relative `//` or backslashes (browsers treat
/// `/\` like `//`).
pub fn is_safe_return_to(path: &str) -> bool {
    path.starts_with('/')
        && !path.starts_with("//")
        && !path.contains('\\')
        && !path.chars().any(char::is_control)
}

#[derive(Serialize, Deserialize)]
struct LoginState {
    nonce: String,
    return_to: String,
}

#[derive(Deserialize)]
struct LoginQuery {
    return_to: Option<String>,
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct DeviceTokenRequest {
    device_code: String,
}

#[derive(Deserialize)]
struct RefreshRequest {
    refresh_token: String,
}

#[derive(Deserialize)]
struct OrganizationRequest {
    name: String,
}

#[derive(Deserialize)]
struct InvitationRequest {
    email: String,
}

#[derive(Deserialize)]
struct BillingQuery {
    portal_return: Option<String>,
}

#[derive(Serialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_at: i64,
}

fn bad_request(message: &str) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, message)
}

fn random_nonce() -> String {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn decode_state(state: &str) -> Option<LoginState> {
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(state).ok()?).ok()
}

async fn login(
    State(workos): State<Arc<WorkOs>>,
    headers: HeaderMap,
    query: Result<Query<LoginQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let Query(query) = query?;
    let return_to = query.return_to.unwrap_or_else(|| "/".to_string());
    if !is_safe_return_to(&return_to) {
        return Err(bad_request("invalid_return_to"));
    }
    let nonce = random_nonce();
    let state = LoginState {
        nonce: nonce.clone(),
        return_to,
    };
    let state = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&state).expect("state serializes"));
    let cookie = Cookie::build((LOGIN_COOKIE, nonce))
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Lax)
        .path(LOGIN_COOKIE_PATH)
        .max_age(cookie::time::Duration::minutes(LOGIN_MAX_AGE_MINUTES))
        .build();
    let jar = workos.cookie_jar(&headers).add(cookie);
    Ok((jar, Redirect::to(&workos.authorize_url(&state))).into_response())
}

async fn callback(
    State(workos): State<Arc<WorkOs>>,
    headers: HeaderMap,
    query: Result<Query<CallbackQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let Query(query) = query?;
    if let Some(error) = query.error {
        return Err(bad_request(&error));
    }
    let jar = workos.cookie_jar(&headers);
    let expected = jar
        .get(LOGIN_COOKIE)
        .ok_or_else(|| bad_request("invalid_state"))?;
    let state = query
        .state
        .as_deref()
        .and_then(decode_state)
        .ok_or_else(|| bad_request("invalid_state"))?;
    if !bool::from(state.nonce.as_bytes().ct_eq(expected.value().as_bytes())) {
        return Err(bad_request("invalid_state"));
    }
    if !is_safe_return_to(&state.return_to) {
        return Err(bad_request("invalid_return_to"));
    }
    let code = query.code.ok_or_else(|| bad_request("missing_code"))?;
    let session = workos.exchange_code(&code).await.map_err(|err| match err {
        WorkOsError::Api(code) => {
            eprintln!("WorkOS code exchange rejected: {code}");
            bad_request(&code)
        }
        WorkOsError::Unavailable => workos_unavailable(),
    })?;
    let jar = jar
        .remove(Cookie::build(LOGIN_COOKIE).path(LOGIN_COOKIE_PATH))
        .add(session_cookie(&session));
    Ok((jar, Redirect::to(&state.return_to)).into_response())
}

const MAX_ORGANIZATION_NAME: usize = 100;
const MAX_EMAIL: usize = 254;

fn organization_name(raw: &str) -> Result<String, ApiError> {
    let name = raw.trim();
    if name.is_empty()
        || name.chars().count() > MAX_ORGANIZATION_NAME
        || name.chars().any(char::is_control)
    {
        return Err(bad_request("invalid_organization_name"));
    }
    Ok(name.to_string())
}

/// One `@`, a domain with a dot, and no whitespace. WorkOS sends the invitation to this address.
fn invitation_email(raw: &str) -> Result<String, ApiError> {
    let email = raw.trim();
    let Some((local, domain)) = email.split_once('@') else {
        return Err(bad_request("invalid_email"));
    };
    if local.is_empty()
        || domain.is_empty()
        || domain.contains('@')
        || !domain.contains('.')
        || email.len() > MAX_EMAIL
        || email.chars().any(char::is_whitespace)
    {
        return Err(bad_request("invalid_email"));
    }
    Ok(email.to_string())
}

async fn create_organization(
    State(workos): State<Arc<WorkOs>>,
    headers: HeaderMap,
    body: Result<Json<OrganizationRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(body) = body?;
    let name = organization_name(&body.name)?;
    let (identity, session) = workos.establish_team(&headers, &name).await?;
    let jar = workos.cookie_jar(&headers).add(session_cookie(&session));
    Ok((jar, Json(identity)).into_response())
}

async fn create_invitation(
    State(workos): State<Arc<WorkOs>>,
    headers: HeaderMap,
    body: Result<Json<InvitationRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(body) = body?;
    let email = invitation_email(&body.email)?;
    let (session_headers, invitation) = workos.invite_member(&headers, &email).await?;
    let mut response = Json(json!({
        "email": invitation.email,
        "state": invitation.state,
    }))
    .into_response();
    response.headers_mut().extend(session_headers);
    Ok(response)
}

async fn billing_status(
    State(workos): State<Arc<WorkOs>>,
    headers: HeaderMap,
    query: Result<Query<BillingQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let Query(query) = query?;
    let (authenticated, claims) = workos.authenticate_session(&headers).await?;
    let portal_return = query.portal_return.as_deref() == Some("1");
    let (summary, refreshed) = workos
        .billing_summary(
            &authenticated.identity,
            &claims.entitlements,
            &headers,
            portal_return,
        )
        .await?;
    let mut response = Json(summary).into_response();
    let extra = if refreshed.is_empty() {
        &authenticated.headers
    } else {
        &refreshed
    };
    response.headers_mut().extend(extra.clone());
    Ok(response)
}

async fn billing_portal(
    State(workos): State<Arc<WorkOs>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (authenticated, _) = workos.authenticate_session(&headers).await?;
    let url = workos.billing_portal(&authenticated.identity).await?;
    Ok(Json(json!({ "url": url })))
}

async fn stripe_webhook(
    State(workos): State<Arc<WorkOs>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let signature = headers
        .get("stripe-signature")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    workos.handle_stripe_webhook(&body, signature).await?;
    Ok(StatusCode::OK)
}

async fn logout(State(workos): State<Arc<WorkOs>>, headers: HeaderMap) -> Response {
    let jar = workos.cookie_jar(&headers);
    let mut logout_url = None;
    if let Some(session) = read_session(&jar) {
        if let Ok(claims) = workos.verify(&session.access_token, false).await {
            logout_url = claims.sid.map(|sid| workos.logout_url(&sid));
        }
    }
    let jar = jar.remove(Cookie::build(SESSION_COOKIE).path("/"));
    let logout_url = logout_url.unwrap_or_else(|| "/".to_string());
    (jar, Json(json!({ "logout_url": logout_url }))).into_response()
}

async fn device_start(
    State(workos): State<Arc<WorkOs>>,
) -> Result<Json<DeviceAuthorization>, ApiError> {
    workos
        .device_start()
        .await
        .map(Json)
        .map_err(|err| match err {
            WorkOsError::Api(code) => {
                eprintln!("WorkOS device authorization rejected: {code}");
                ApiError::new(StatusCode::BAD_GATEWAY, code)
            }
            WorkOsError::Unavailable => workos_unavailable(),
        })
}

async fn device_token(
    State(workos): State<Arc<WorkOs>>,
    body: Result<Json<DeviceTokenRequest>, JsonRejection>,
) -> Result<Json<TokenResponse>, ApiError> {
    let Json(request) = body?;
    match workos.device_token(&request.device_code).await {
        Ok(session) => token_response(&workos, session).await,
        Err(WorkOsError::Api(code)) => {
            let status = match code.as_str() {
                "authorization_pending" => StatusCode::PRECONDITION_REQUIRED,
                "slow_down" => StatusCode::TOO_MANY_REQUESTS,
                _ => StatusCode::BAD_REQUEST,
            };
            Err(ApiError::new(status, code))
        }
        Err(WorkOsError::Unavailable) => Err(workos_unavailable()),
    }
}

async fn refresh(
    State(workos): State<Arc<WorkOs>>,
    body: Result<Json<RefreshRequest>, JsonRejection>,
) -> Result<Json<TokenResponse>, ApiError> {
    let Json(request) = body?;
    match workos.refresh(&request.refresh_token).await {
        Ok(session) => token_response(&workos, session).await,
        Err(WorkOsError::Api(_)) => Err(ApiError::unauthorized()),
        Err(WorkOsError::Unavailable) => Err(workos_unavailable()),
    }
}

async fn token_response(
    workos: &WorkOs,
    session: Session,
) -> Result<Json<TokenResponse>, ApiError> {
    let claims = workos
        .verify(&session.access_token, false)
        .await
        .map_err(|err| {
            eprintln!("WorkOS returned an access token that failed verification: {err:?}");
            workos_unavailable()
        })?;
    Ok(Json(TokenResponse {
        access_token: session.access_token,
        refresh_token: session.refresh_token,
        expires_at: claims.exp,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_to_must_be_a_same_origin_path() {
        for ok in ["/", "/transcripts", "/a/b?c=d#e", "/%2F%2Fevil.com"] {
            assert!(is_safe_return_to(ok), "{ok}");
        }
        for bad in [
            "",
            "transcripts",
            "//evil.com",
            "/\\evil.com",
            "https://evil.com",
            "javascript:alert(1)",
            "/ok\r\nSet-Cookie: x",
        ] {
            assert!(!is_safe_return_to(bad), "{bad}");
        }
    }

    #[test]
    fn state_round_trips() {
        let state = LoginState {
            nonce: "n".to_string(),
            return_to: "/x".to_string(),
        };
        let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&state).unwrap());
        let decoded = decode_state(&encoded).unwrap();
        assert_eq!(decoded.nonce, "n");
        assert_eq!(decoded.return_to, "/x");
        assert!(decode_state("not base64!").is_none());
    }

    #[test]
    fn organization_name_is_trimmed_and_bounded() {
        assert_eq!(organization_name("  Company A  ").unwrap(), "Company A");
        assert!(organization_name("   ").is_err());
        assert!(organization_name(&"a".repeat(MAX_ORGANIZATION_NAME + 1)).is_err());
        assert!(organization_name("line\nbreak").is_err());
    }

    #[test]
    fn invitation_email_requires_a_domain() {
        assert_eq!(
            invitation_email("  Dev@CompanyA.example ").unwrap(),
            "Dev@CompanyA.example"
        );
        for bad in [
            "",
            "dev",
            "dev@",
            "@company.example",
            "dev@company",
            "a b@company.example",
        ] {
            assert!(invitation_email(bad).is_err(), "{bad}");
        }
    }
}
