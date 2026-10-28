use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use axum::http::{header, HeaderMap};
use rusqlite::Connection;
use serde::Serialize;
use subtle::ConstantTimeEq;

use crate::ApiError;

pub const DEFAULT_ORG: &str = "default";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Identity {
    pub user_id: String,
    pub email: Option<String>,
    pub org_id: String,
    pub org_name: Option<String>,
}

/// A successful authentication plus headers (e.g. a refreshed `Set-Cookie`) to append to the
/// response.
#[derive(Debug)]
pub struct Authenticated {
    pub identity: Identity,
    pub headers: HeaderMap,
}

impl From<Identity> for Authenticated {
    fn from(identity: Identity) -> Self {
        Authenticated {
            identity,
            headers: HeaderMap::new(),
        }
    }
}

pub type AuthFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Authenticated, ApiError>> + Send + 'a>>;

pub trait Authenticator: Send + Sync {
    /// Reported by `GET /api/auth/config`: "none", "token" or "workos".
    fn mode(&self) -> &'static str;
    fn authenticate<'a>(&'a self, headers: &'a HeaderMap) -> AuthFuture<'a>;

    /// The hosted edition stores subscription state in the engine database. Other editions ignore it.
    fn bind_db(&self, _db: Arc<Mutex<Connection>>) {}

    /// `authenticate`, then any edition-specific access check for `path`.
    /// The default allows every authenticated caller.
    fn authorize<'a>(&'a self, headers: &'a HeaderMap, path: &'a str) -> AuthFuture<'a> {
        let _ = path;
        self.authenticate(headers)
    }
}

pub struct NoAuth;

impl Authenticator for NoAuth {
    fn mode(&self) -> &'static str {
        "none"
    }

    fn authenticate<'a>(&'a self, _headers: &'a HeaderMap) -> AuthFuture<'a> {
        Box::pin(async { Ok(default_identity("default").into()) })
    }
}

pub struct TokenAuth {
    token: String,
}

impl TokenAuth {
    pub fn new(token: impl Into<String>) -> Self {
        TokenAuth {
            token: token.into(),
        }
    }
}

impl Authenticator for TokenAuth {
    fn mode(&self) -> &'static str {
        "token"
    }

    fn authenticate<'a>(&'a self, headers: &'a HeaderMap) -> AuthFuture<'a> {
        Box::pin(async move {
            let presented = bearer_token(headers).ok_or_else(ApiError::unauthorized)?;
            if bool::from(presented.as_bytes().ct_eq(self.token.as_bytes())) {
                Ok(default_identity("token").into())
            } else {
                Err(ApiError::unauthorized())
            }
        })
    }
}

fn default_identity(user_id: &str) -> Identity {
    Identity {
        user_id: user_id.to_string(),
        email: None,
        org_id: DEFAULT_ORG.to_string(),
        org_name: None,
    }
}

/// `CADRI_API_TOKEN` set: static bearer token; unset or empty: no authentication.
pub fn from_env() -> Arc<dyn Authenticator> {
    match std::env::var("CADRI_API_TOKEN") {
        Ok(token) if !token.trim().is_empty() => Arc::new(TokenAuth::new(token.trim())),
        _ => {
            eprintln!("**********************************************************************");
            eprintln!("WARNING: CADRI_API_TOKEN is not set; the API accepts unauthenticated");
            eprintln!("requests. Anyone who can reach this server can read and write data.");
            eprintln!("**********************************************************************");
            Arc::new(NoAuth)
        }
    }
}

pub fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(authorization: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(authorization).unwrap(),
        );
        headers
    }

    #[test]
    fn bearer_token_parses_the_scheme_case_insensitively() {
        assert_eq!(bearer_token(&headers("Bearer abc")), Some("abc"));
        assert_eq!(bearer_token(&headers("bearer  abc ")), Some("abc"));
        assert_eq!(bearer_token(&headers("Basic abc")), None);
        assert_eq!(bearer_token(&headers("Bearer ")), None);
        assert_eq!(bearer_token(&HeaderMap::new()), None);
    }

    #[tokio::test]
    async fn token_auth_requires_the_exact_token() {
        let auth = TokenAuth::new("secret");
        assert!(auth.authenticate(&headers("Bearer secret")).await.is_ok());
        assert!(auth.authenticate(&headers("Bearer secre")).await.is_err());
        assert!(auth.authenticate(&headers("Bearer secret2")).await.is_err());
        assert!(auth.authenticate(&HeaderMap::new()).await.is_err());
    }
}
