use std::collections::HashMap;
use std::fmt::Display;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::{Cookie, Key, PrivateCookieJar, SameSite};
use base64::Engine;
use cadri_core::auth::bearer_token;
use cadri_core::{ApiError, AuthFuture, Authenticated, Authenticator, Identity};
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const SESSION_COOKIE: &str = "cadri_session";
const DEFAULT_API_BASE: &str = "https://api.workos.com";
const JWKS_MIN_REFETCH: Duration = Duration::from_secs(300);
const PROFILE_TTL: Duration = Duration::from_secs(300);
// Concurrent requests carrying the same expired cookie must not each spend the single-use
// refresh token.
const REFRESH_REUSE: Duration = Duration::from_secs(60);
const SESSION_MAX_AGE_DAYS: i64 = 30;

#[derive(Clone)]
pub struct Config {
    pub api_key: String,
    pub client_id: String,
    pub cookie_key: Key,
    /// No trailing slash, e.g. `https://staging.cadri.io`.
    pub public_url: String,
    /// No trailing slash; `WORKOS_API_BASE`, default `https://api.workos.com`.
    pub api_base: String,
}

impl Config {
    pub fn from_env() -> Result<Config, String> {
        let get = |name: &str| {
            std::env::var(name)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        let mut missing = Vec::new();
        let mut required = |name: &'static str| {
            get(name).unwrap_or_else(|| {
                missing.push(name);
                String::new()
            })
        };
        let api_key = required("WORKOS_API_KEY");
        let client_id = required("WORKOS_CLIENT_ID");
        let cookie_key = required("CADRI_COOKIE_KEY");
        let public_url = required("CADRI_PUBLIC_URL");
        if !missing.is_empty() {
            return Err(format!(
                "missing required environment variables: {}",
                missing.join(", ")
            ));
        }
        let api_base = get("WORKOS_API_BASE").unwrap_or_else(|| DEFAULT_API_BASE.to_string());
        for (name, value) in [
            ("CADRI_PUBLIC_URL", &public_url),
            ("WORKOS_API_BASE", &api_base),
        ] {
            reqwest::Url::parse(value)
                .map_err(|err| format!("{name} is not a valid URL: {err}"))?;
        }
        Ok(Config {
            api_key,
            client_id,
            cookie_key: cookie_key_from_base64(&cookie_key)?,
            public_url: public_url.trim_end_matches('/').to_string(),
            api_base: api_base.trim_end_matches('/').to_string(),
        })
    }
}

pub fn cookie_key_from_base64(encoded: &str) -> Result<Key, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|err| format!("CADRI_COOKIE_KEY is not valid base64: {err}"))?;
    if bytes.len() < 64 {
        return Err(format!(
            "CADRI_COOKIE_KEY must decode to at least 64 bytes, got {}",
            bytes.len()
        ));
    }
    Ok(Key::from(&bytes))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Session {
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Invitation {
    pub email: String,
    pub state: String,
}

enum Membership {
    None,
    One(String),
    Many,
}

#[derive(Deserialize)]
struct WorkOsOrganization {
    id: String,
}

#[derive(Deserialize)]
struct WorkOsMembership {
    organization_id: String,
    #[serde(default)]
    status: String,
}

#[derive(Deserialize)]
struct MembershipPage {
    #[serde(default)]
    data: Vec<WorkOsMembership>,
    #[serde(default)]
    list_metadata: ListMetadata,
}

#[derive(Default, Deserialize)]
struct ListMetadata {
    after: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Claims {
    pub sub: String,
    pub org_id: Option<String>,
    pub sid: Option<String>,
    pub exp: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct DeviceAuthorization {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Debug)]
pub(crate) enum WorkOsError {
    /// WorkOS answered with an error code (`error` or `code` field).
    Api(String),
    Unavailable,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TokenError {
    Expired,
    Invalid,
    Unavailable,
}

#[derive(Default, Deserialize)]
struct ErrorBody {
    error: Option<String>,
    code: Option<String>,
}

#[derive(Default)]
struct Jwks {
    keys: HashMap<String, DecodingKey>,
    fetched_at: Option<Instant>,
}

pub struct WorkOs {
    config: Config,
    http: reqwest::Client,
    issuers: [String; 2],
    jwks: tokio::sync::Mutex<Jwks>,
    refreshes: tokio::sync::Mutex<HashMap<String, (Instant, Session)>>,
    profiles: Mutex<HashMap<String, (Instant, Option<String>)>>,
}

impl WorkOs {
    pub fn new(config: Config) -> Result<WorkOs, String> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|err| format!("failed to build HTTP client: {err}"))?;
        // WorkOS documents both forms; AuthKit tokens use the per-client one.
        let issuers = [
            config.api_base.clone(),
            format!("{}/user_management/{}", config.api_base, config.client_id),
        ];
        Ok(WorkOs {
            config,
            http,
            issuers,
            jwks: Default::default(),
            refreshes: Default::default(),
            profiles: Default::default(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.config.api_base)
    }

    pub(crate) fn authorize_url(&self, state: &str) -> String {
        let redirect_uri = format!("{}/auth/callback", self.config.public_url);
        reqwest::Url::parse_with_params(
            &self.url("/user_management/authorize"),
            [
                ("provider", "authkit"),
                ("client_id", self.config.client_id.as_str()),
                ("redirect_uri", redirect_uri.as_str()),
                ("response_type", "code"),
                ("state", state),
            ],
        )
        .expect("WorkOS API base is a valid URL")
        .into()
    }

    pub(crate) fn logout_url(&self, session_id: &str) -> String {
        reqwest::Url::parse_with_params(
            &self.url("/user_management/sessions/logout"),
            [("session_id", session_id)],
        )
        .expect("WorkOS API base is a valid URL")
        .into()
    }

    pub(crate) fn cookie_jar(&self, headers: &HeaderMap) -> PrivateCookieJar {
        PrivateCookieJar::from_headers(headers, self.config.cookie_key.clone())
    }

    pub(crate) async fn exchange_code(&self, code: &str) -> Result<Session, WorkOsError> {
        self.authenticate_grant(json!({
            "grant_type": "authorization_code",
            "client_id": self.config.client_id,
            "client_secret": self.config.api_key,
            "code": code,
        }))
        .await
    }

    pub(crate) async fn refresh(&self, refresh_token: &str) -> Result<Session, WorkOsError> {
        self.authenticate_grant(json!({
            "grant_type": "refresh_token",
            "client_id": self.config.client_id,
            "client_secret": self.config.api_key,
            "refresh_token": refresh_token,
        }))
        .await
    }

    /// Issues a new access token that carries `organization_id`. WorkOS rotates the refresh token.
    pub(crate) async fn refresh_into(
        &self,
        refresh_token: &str,
        organization_id: &str,
    ) -> Result<Session, WorkOsError> {
        self.authenticate_grant(json!({
            "grant_type": "refresh_token",
            "client_id": self.config.client_id,
            "client_secret": self.config.api_key,
            "refresh_token": refresh_token,
            "organization_id": organization_id,
        }))
        .await
    }

    pub(crate) async fn device_token(&self, device_code: &str) -> Result<Session, WorkOsError> {
        self.authenticate_grant(json!({
            "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
            "client_id": self.config.client_id,
            "device_code": device_code,
        }))
        .await
    }

    pub(crate) async fn device_start(&self) -> Result<DeviceAuthorization, WorkOsError> {
        let response = self
            .http
            .post(self.url("/user_management/authorize/device"))
            .json(&json!({ "client_id": self.config.client_id }))
            .send()
            .await
            .map_err(|err| unavailable("device authorization", err))?;
        parse_response(response, "device authorization").await
    }

    async fn authenticate_grant(&self, body: Value) -> Result<Session, WorkOsError> {
        let response = self
            .http
            .post(self.url("/user_management/authenticate"))
            .json(&body)
            .send()
            .await
            .map_err(|err| unavailable("authenticate", err))?;
        parse_response(response, "authenticate").await
    }

    /// Verifies signature and issuer; `exp` is checked only when `check_exp` is set.
    pub(crate) async fn verify(&self, token: &str, check_exp: bool) -> Result<Claims, TokenError> {
        let header = jsonwebtoken::decode_header(token).map_err(|_| TokenError::Invalid)?;
        let kid = header.kid.ok_or(TokenError::Invalid)?;
        let key = self.decoding_key(&kid).await?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&self.issuers);
        validation.set_required_spec_claims(&["exp", "sub", "iss"]);
        validation.validate_aud = false;
        validation.validate_exp = check_exp;
        match jsonwebtoken::decode::<Claims>(token, &key, &validation) {
            Ok(data) => Ok(data.claims),
            Err(err) if matches!(err.kind(), ErrorKind::ExpiredSignature) => {
                Err(TokenError::Expired)
            }
            Err(_) => Err(TokenError::Invalid),
        }
    }

    async fn decoding_key(&self, kid: &str) -> Result<DecodingKey, TokenError> {
        let mut jwks = self.jwks.lock().await;
        if let Some(key) = jwks.keys.get(kid) {
            return Ok(key.clone());
        }
        if jwks
            .fetched_at
            .is_some_and(|at| at.elapsed() < JWKS_MIN_REFETCH)
        {
            return Err(TokenError::Invalid);
        }
        match self.fetch_jwks().await {
            Ok(keys) => {
                jwks.keys = keys;
                jwks.fetched_at = Some(Instant::now());
            }
            Err(err) => {
                eprintln!("WorkOS JWKS fetch failed: {err}");
                return Err(TokenError::Unavailable);
            }
        }
        jwks.keys.get(kid).cloned().ok_or(TokenError::Invalid)
    }

    async fn fetch_jwks(&self) -> Result<HashMap<String, DecodingKey>, String> {
        let set: JwkSet = self
            .http
            .get(self.url(&format!("/sso/jwks/{}", self.config.client_id)))
            .send()
            .await
            .and_then(|response| response.error_for_status())
            .map_err(|err| err.to_string())?
            .json()
            .await
            .map_err(|err| err.to_string())?;
        Ok(set
            .keys
            .iter()
            .filter_map(|jwk| {
                let kid = jwk.common.key_id.clone()?;
                Some((kid, DecodingKey::from_jwk(jwk).ok()?))
            })
            .collect())
    }

    async fn identity(&self, claims: Claims) -> Result<Identity, ApiError> {
        let org_id = claims
            .org_id
            .filter(|org_id| !org_id.is_empty())
            .ok_or_else(|| ApiError::new(StatusCode::FORBIDDEN, "no_organization"))?;
        let (email, org_name) = tokio::join!(
            self.profile_field(format!("/user_management/users/{}", claims.sub), "email"),
            self.profile_field(format!("/organizations/{org_id}"), "name"),
        );
        Ok(Identity {
            user_id: claims.sub,
            email,
            org_id,
            org_name,
        })
    }

    async fn profile_field(&self, path: String, field: &str) -> Option<String> {
        {
            let profiles = lock(&self.profiles);
            if let Some((at, value)) = profiles.get(&path) {
                if at.elapsed() < PROFILE_TTL {
                    return value.clone();
                }
            }
        }
        let value = self.fetch_profile_field(&path, field).await;
        lock(&self.profiles).insert(path, (Instant::now(), value.clone()));
        value
    }

    async fn fetch_profile_field(&self, path: &str, field: &str) -> Option<String> {
        let response = self
            .http
            .get(self.url(path))
            .bearer_auth(&self.config.api_key)
            .send()
            .await
            .and_then(|response| response.error_for_status());
        let body: Value = match response {
            Ok(response) => response.json().await.ok()?,
            Err(err) => {
                eprintln!("WorkOS lookup {path} failed: {err}");
                return None;
            }
        };
        body.get(field)?.as_str().map(str::to_string)
    }

    async fn refresh_session(&self, refresh_token: &str) -> Result<Session, ApiError> {
        let mut recent = self.refreshes.lock().await;
        recent.retain(|_, (at, _)| at.elapsed() < REFRESH_REUSE);
        if let Some((_, session)) = recent.get(refresh_token) {
            return Ok(session.clone());
        }
        let session = self.refresh(refresh_token).await.map_err(|err| match err {
            WorkOsError::Api(_) => ApiError::unauthorized(),
            WorkOsError::Unavailable => workos_unavailable(),
        })?;
        recent.insert(refresh_token.to_string(), (Instant::now(), session.clone()));
        Ok(session)
    }

    /// Gives the signed-in user one organization and a session token that carries it.
    ///
    /// AuthKit can register a user who was not invited. That access token has no `org_id`, and
    /// Cadri stores every transcript under the organization on the token. This creates the
    /// organization they name and adds them as its admin. An email invitation joins an existing
    /// organization during signup, so the invitee never reaches here. A token that already names
    /// an organization is not given a second one. Someone with several memberships is rejected:
    /// there is no organization switcher.
    pub(crate) async fn establish_team(
        &self,
        headers: &HeaderMap,
        name: &str,
    ) -> Result<(Identity, Session), ApiError> {
        let session = read_session(&self.cookie_jar(headers)).ok_or_else(ApiError::unauthorized)?;
        let claims = self
            .verify(&session.access_token, false)
            .await
            .map_err(token_error)?;
        if let Some(org_id) = nonempty(&claims.org_id) {
            match self.verify(&session.access_token, true).await {
                Ok(_) => {
                    return Err(ApiError::new(
                        StatusCode::CONFLICT,
                        "already_in_organization",
                    ))
                }
                Err(TokenError::Expired) => {
                    let fresh = self
                        .refresh_into(&session.refresh_token, org_id)
                        .await
                        .map_err(workos_call)?;
                    return self.ready(fresh).await;
                }
                Err(err) => return Err(token_error(err)),
            }
        }
        let org_id = match self.memberships(&claims.sub).await.map_err(workos_call)? {
            Membership::None => {
                let org = self.create_organization(name).await.map_err(workos_call)?;
                self.create_admin(&claims.sub, &org.id)
                    .await
                    .map_err(workos_call)?;
                org.id
            }
            Membership::One(org_id) => org_id,
            Membership::Many => {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "multiple_organizations",
                ))
            }
        };
        let fresh = self
            .refresh_into(&session.refresh_token, &org_id)
            .await
            .map_err(workos_call)?;
        self.ready(fresh).await
    }

    /// Sends a WorkOS invitation email. The organization is the one on the session token.
    pub(crate) async fn invite_member(
        &self,
        headers: &HeaderMap,
        email: &str,
    ) -> Result<(HeaderMap, Invitation), ApiError> {
        let authenticated = self.authenticate_session(headers).await?;
        let invitation = self
            .send_invitation(
                email,
                &authenticated.identity.org_id,
                &authenticated.identity.user_id,
            )
            .await
            .map_err(workos_call)?;
        Ok((authenticated.headers, invitation))
    }

    async fn ready(&self, session: Session) -> Result<(Identity, Session), ApiError> {
        let claims = self
            .verify(&session.access_token, true)
            .await
            .map_err(token_error)?;
        let identity = self.identity(claims).await?;
        Ok((identity, session))
    }

    async fn memberships(&self, user_id: &str) -> Result<Membership, WorkOsError> {
        let url = reqwest::Url::parse_with_params(
            &self.url("/user_management/organization_memberships"),
            [("user_id", user_id), ("limit", "10")],
        )
        .expect("WorkOS API base is a valid URL");
        let response = self
            .http
            .get(url)
            .bearer_auth(&self.config.api_key)
            .send()
            .await
            .map_err(|err| unavailable("organization memberships", err))?;
        let page: MembershipPage = parse_response(response, "organization memberships").await?;
        let mut orgs: Vec<String> = page
            .data
            .into_iter()
            .filter(|membership| membership.status == "active")
            .map(|membership| membership.organization_id)
            .collect();
        orgs.sort();
        orgs.dedup();
        let more = page
            .list_metadata
            .after
            .is_some_and(|cursor| !cursor.is_empty());
        Ok(match (orgs.len(), more) {
            (0, false) => Membership::None,
            (1, false) => Membership::One(orgs.remove(0)),
            _ => Membership::Many,
        })
    }

    async fn create_organization(&self, name: &str) -> Result<WorkOsOrganization, WorkOsError> {
        let response = self
            .http
            .post(self.url("/organizations"))
            .bearer_auth(&self.config.api_key)
            .json(&json!({ "name": name }))
            .send()
            .await
            .map_err(|err| unavailable("create organization", err))?;
        parse_response(response, "create organization").await
    }

    async fn create_admin(&self, user_id: &str, organization_id: &str) -> Result<(), WorkOsError> {
        let response = self
            .http
            .post(self.url("/user_management/organization_memberships"))
            .bearer_auth(&self.config.api_key)
            .json(&json!({
                "user_id": user_id,
                "organization_id": organization_id,
                "role_slug": "admin",
            }))
            .send()
            .await
            .map_err(|err| unavailable("create organization membership", err))?;
        let _: Value = parse_response(response, "create organization membership").await?;
        Ok(())
    }

    async fn send_invitation(
        &self,
        email: &str,
        organization_id: &str,
        inviter_user_id: &str,
    ) -> Result<Invitation, WorkOsError> {
        let response = self
            .http
            .post(self.url("/user_management/invitations"))
            .bearer_auth(&self.config.api_key)
            .json(&json!({
                "email": email,
                "organization_id": organization_id,
                "inviter_user_id": inviter_user_id,
            }))
            .send()
            .await
            .map_err(|err| unavailable("invitation", err))?;
        parse_response(response, "invitation").await
    }

    async fn authenticate_session(&self, headers: &HeaderMap) -> Result<Authenticated, ApiError> {
        if let Some(token) = bearer_token(headers) {
            let claims = self.verify(token, true).await.map_err(token_error)?;
            return Ok(self.identity(claims).await?.into());
        }
        let session = read_session(&self.cookie_jar(headers)).ok_or_else(ApiError::unauthorized)?;
        match self.verify(&session.access_token, true).await {
            Ok(claims) => Ok(self.identity(claims).await?.into()),
            Err(TokenError::Expired) => {
                let fresh = self.refresh_session(&session.refresh_token).await?;
                let claims = self
                    .verify(&fresh.access_token, true)
                    .await
                    .map_err(token_error)?;
                let identity = self.identity(claims).await?;
                let jar = PrivateCookieJar::new(self.config.cookie_key.clone())
                    .add(session_cookie(&fresh));
                Ok(Authenticated {
                    identity,
                    headers: jar.into_response().into_parts().0.headers,
                })
            }
            Err(err) => Err(token_error(err)),
        }
    }
}

impl Authenticator for WorkOs {
    fn mode(&self) -> &'static str {
        "workos"
    }

    fn authenticate<'a>(&'a self, headers: &'a HeaderMap) -> AuthFuture<'a> {
        Box::pin(self.authenticate_session(headers))
    }
}

pub(crate) fn read_session(jar: &PrivateCookieJar) -> Option<Session> {
    serde_json::from_str(jar.get(SESSION_COOKIE)?.value()).ok()
}

pub(crate) fn session_cookie(session: &Session) -> Cookie<'static> {
    let value = serde_json::to_string(session).expect("session serializes");
    Cookie::build((SESSION_COOKIE, value))
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(cookie::time::Duration::days(SESSION_MAX_AGE_DAYS))
        .build()
}

pub(crate) fn workos_unavailable() -> ApiError {
    ApiError::new(StatusCode::BAD_GATEWAY, "workos_unavailable")
}

fn nonempty(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn workos_call(err: WorkOsError) -> ApiError {
    match err {
        WorkOsError::Unavailable => workos_unavailable(),
        WorkOsError::Api(code) => {
            eprintln!("WorkOS rejected the request: {code}");
            match code.as_str() {
                "user_already_organization_member" => {
                    ApiError::new(StatusCode::CONFLICT, "already_member")
                }
                "invitation_already_exists" | "invite_already_sent" | "email_already_invited" => {
                    ApiError::new(StatusCode::CONFLICT, "already_invited")
                }
                _ => ApiError::new(StatusCode::BAD_GATEWAY, "workos_rejected"),
            }
        }
    }
}

fn token_error(err: TokenError) -> ApiError {
    match err {
        TokenError::Expired | TokenError::Invalid => ApiError::unauthorized(),
        TokenError::Unavailable => workos_unavailable(),
    }
}

fn unavailable(what: &str, err: impl Display) -> WorkOsError {
    eprintln!("WorkOS {what} request failed: {err}");
    WorkOsError::Unavailable
}

async fn parse_response<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    what: &str,
) -> Result<T, WorkOsError> {
    let status = response.status();
    if !status.is_success() {
        let body: ErrorBody = response.json().await.unwrap_or_default();
        let code = body
            .error
            .or(body.code)
            .unwrap_or_else(|| format!("http_{}", status.as_u16()));
        return Err(WorkOsError::Api(code));
    }
    response.json().await.map_err(|err| unavailable(what, err))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
