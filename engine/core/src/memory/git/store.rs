//! Legacy directory-store layout (`refs/heads/main` plus full-history bundles under
//! `objects/`). Only read, to migrate a book to the log format.

use std::fs;
use std::path::{Path, PathBuf};

use super::error::Fail;
use super::git_cli_exec::{git_ok, Repo};
use super::ids::is_hex;

pub(super) fn ref_path(store: &Path, repo_id: &str) -> PathBuf {
    store.join("repos").join(repo_id).join("refs/heads/main")
}

pub(super) fn obj_path(store: &Path, repo_id: &str, sha: &str) -> PathBuf {
    store.join("repos").join(repo_id).join("objects").join(sha)
}

pub(super) fn clear_stage(repo: &Repo) {
    let _ = git_ok(repo, &["update-ref", "-d", "refs/cadri/stage"], None, &[]);
}

pub(super) fn read_sha_file(path: &Path) -> Result<Option<String>, Fail> {
    match fs::read_to_string(path) {
        Ok(text) => {
            let sha = text.trim();
            if sha.is_empty() {
                Ok(None)
            } else if is_hex(sha, 40) {
                Ok(Some(sha.to_string()))
            } else {
                Err(Fail::git(format!("bad memory ref in {}", path.display())))
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(Fail::git(err.to_string())),
    }
}
