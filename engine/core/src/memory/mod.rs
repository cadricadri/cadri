//! Team and personal memory books (git-backed object store).
//!
//! Submodules are the domain pieces: JSON types, git service, S3/directory
//! publish, Landlock jail, and the testing seam used by `tests/memory_*.rs`.

mod git;
mod s3;
mod types;

pub(crate) mod jail;
pub mod testing;

pub(crate) use git::GitHttp;
pub use git::{memory_repo_id, MemoryService, PublishGate};
pub use s3::{FakeS3, RefFault, S3Books};
pub(crate) use types::{BlameEntry, Change, MemoryCommit, MemoryStore, MemoryTree};

pub fn jail_available() -> bool {
    jail::available()
}
