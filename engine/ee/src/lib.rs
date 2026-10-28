mod routes;
mod stripe;
mod workos;

use std::sync::Arc;

use axum::Router;
use cadri_core::Authenticator;

pub use routes::{is_safe_return_to, router, LOGIN_COOKIE};
pub use stripe::{StripeConfig, API_VERSION};
pub use workos::{cookie_key_from_base64, sign_webhook, Config, WorkOs, SESSION_COOKIE};

/// Reads the WorkOS configuration from the environment and returns the authenticator plus the
/// `/auth/*` routes to pass to `cadri_core::serve`.
pub fn from_env() -> Result<(Arc<dyn Authenticator>, Router), String> {
    let workos = Arc::new(WorkOs::with_stripe(
        Config::from_env()?,
        StripeConfig::from_env()?,
    )?);
    Ok((workos.clone(), router(workos)))
}
