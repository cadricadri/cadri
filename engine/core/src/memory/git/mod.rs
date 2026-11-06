//! Git-backed memory store: UI commits, smart HTTP, and bundle publish.
//!
//! Durable state is one tip ref plus immutable bundles (directory or S3). A local
//! bare repo is the working copy where `git` validates packs, builds commits,
//! creates bundles, serves upload-pack, and computes blame. That is production
//! path for both local and hosted — not a test-only helper.

mod author;
mod bundle;
mod commit;
mod durable;
mod error;
mod git_cli_exec;
mod ids;
mod protocol;
mod service;
mod snapshot;
mod store;
mod tree;
mod view;

#[cfg(test)]
mod tests;

pub use ids::memory_repo_id;
pub(crate) use protocol::GitHttp;
pub use service::{MemoryService, PublishGate};
