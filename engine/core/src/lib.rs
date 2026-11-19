//! Cadri engine library: auth, sessions DB, and git-backed memory.

pub mod auth;
pub mod db;
pub mod memory;
pub mod transcript;

mod api;
mod database;
mod error;
mod pg;
pub mod schema;
mod telemetry;

pub use database::Database;

pub use telemetry::{
    observe_billing_transition, observe_signup, reset_org_statuses, set_org_status,
    set_scrape_hook, time_postgres,
};

pub use api::{assemble, router, serve};
pub use auth::{AuthFuture, Authenticated, Authenticator, Identity};
pub use error::ApiError;

/// Seam for `tests/memory_*.rs` (integration tests cannot see `pub(crate)`).
pub use memory::testing as memory_testing;

pub fn memory_jail_available() -> bool {
    memory::jail_available()
}
