use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::error::Fail;
use super::git_cli_exec::{git_ok, Repo};
use super::ids::is_hex;

pub(super) fn ref_path(store: &Path, repo_id: &str) -> PathBuf {
    store.join("repos").join(repo_id).join("refs/heads/main")
}

pub(super) fn obj_path(store: &Path, repo_id: &str, sha: &str) -> PathBuf {
    store.join("repos").join(repo_id).join("objects").join(sha)
}

pub(super) fn put_bundle_file(path: &Path, bytes: &[u8]) -> Result<(), Fail> {
    if path.is_file() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| Fail::git(err.to_string()))?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).map_err(|err| Fail::git(err.to_string()))?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(_) if path.is_file() => Ok(()),
        Err(err) => Err(Fail::git(err.to_string())),
    }
}

pub(super) fn cas_ref_file(path: &Path, expected: Option<&str>, new_sha: &str) -> Result<(), Fail> {
    let dir = path.parent().ok_or_else(|| Fail::git("memory ref path"))?;
    fs::create_dir_all(dir).map_err(|err| Fail::git(err.to_string()))?;
    let lock_path = dir.join("main.lock");
    let mut held = None;
    for _ in 0..40 {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(file) => {
                held = Some(file);
                break;
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if stale_lock(&lock_path) {
                    let _ = fs::remove_file(&lock_path);
                } else {
                    std::thread::sleep(Duration::from_millis(15));
                }
            }
            Err(err) => return Err(Fail::git(err.to_string())),
        }
    }
    let Some(_held) = held else {
        return Err(Fail::Conflict);
    };
    let result = (|| {
        let current = read_sha_file(path)?;
        if current.as_deref() != expected {
            return Err(Fail::Conflict);
        }
        let tmp = dir.join(format!(".{new_sha}.tmp"));
        fs::write(&tmp, format!("{new_sha}\n")).map_err(|err| Fail::git(err.to_string()))?;
        fs::rename(&tmp, path).map_err(|err| Fail::git(err.to_string()))?;
        Ok(())
    })();
    let _ = fs::remove_file(&lock_path);
    result
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

fn stale_lock(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age > Duration::from_secs(15))
}
