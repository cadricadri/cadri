//! Surfaces used by `tests/memory_*.rs`.
//!
//! Integration tests are a separate crate, so they cannot see `pub(crate)` items.
//! This module is the deliberate seam for those suites — not a stable product API.

pub use crate::assemble;
pub use crate::memory::{memory_repo_id, FakeS3, LogFault, MemoryService, PublishGate, S3Books};
