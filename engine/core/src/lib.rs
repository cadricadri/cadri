//! Cadri engine library: auth, transcripts DB, and git-backed memory.

pub mod auth;
pub mod db;
pub mod memory;
pub mod transcript;

mod api;
mod error;
mod telemetry;

pub use api::{assemble, router, serve};
pub use auth::{AuthFuture, Authenticated, Authenticator, Identity};
pub use error::ApiError;

/// Seam for `tests/memory_*.rs` (integration tests cannot see `pub(crate)`).
pub use memory::testing as memory_testing;

pub fn memory_jail_available() -> bool {
    memory::jail_available()
}
