//! Spawn the system `git` CLI against a bare repo directory (optionally
//! landlock-jailed).
//!
//! The durable store is bundles + a tip ref (directory or S3). The engine still
//! needs a real git process to build commits, validate packs, create bundles,
//! run upload-pack, and compute blame — S3 alone is opaque bytes.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use super::error::Fail;
use super::ids::is_hex;

#[cfg(test)]
thread_local! {
    /// Git processes started by this thread, for tests that bound the count.
    pub(super) static SPAWNS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(super) struct Repo {
    pub(super) git_dir: PathBuf,
    pub(super) scratch: PathBuf,
    pub(super) jail: bool,
}
pub(super) struct GitOutput {
    pub(super) ok: bool,
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: String,
}

pub(super) fn git_text(
    repo: &Repo,
    args: &[&str],
    stdin: Option<&[u8]>,
    env: &[(&str, &str)],
) -> Result<String, Fail> {
    let bytes = git_bytes(repo, args, stdin, env)?;
    Ok(String::from_utf8_lossy(&bytes).trim().to_string())
}

pub(super) fn git_bytes(
    repo: &Repo,
    args: &[&str],
    stdin: Option<&[u8]>,
    env: &[(&str, &str)],
) -> Result<Vec<u8>, Fail> {
    let out = git_out(repo, args, stdin, env)?;
    if out.ok {
        Ok(out.stdout)
    } else {
        Err(Fail::git(format!(
            "{}: {}",
            args.first().copied().unwrap_or("git"),
            out.stderr
        )))
    }
}

pub(super) fn git_ok(
    repo: &Repo,
    args: &[&str],
    stdin: Option<&[u8]>,
    env: &[(&str, &str)],
) -> Result<(), Fail> {
    git_bytes(repo, args, stdin, env).map(|_| ())
}

pub(super) fn git_out(
    repo: &Repo,
    args: &[&str],
    stdin: Option<&[u8]>,
    env: &[(&str, &str)],
) -> Result<GitOutput, Fail> {
    #[cfg(test)]
    SPAWNS.with(|count| count.set(count.get() + 1));
    let mut command = if repo.jail {
        let exe = std::env::current_exe().map_err(|err| Fail::git(err.to_string()))?;
        let mut command = Command::new(exe);
        command
            .arg("--memory-confine")
            .arg(&repo.git_dir)
            .arg("--memory-scratch")
            .arg(&repo.scratch)
            .arg("--")
            .arg("git");
        command
    } else {
        Command::new("git")
    };
    command
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("TMPDIR", &repo.scratch)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .args([
            "-c",
            "gc.auto=0",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.quotepath=false",
            "-c",
            "protocol.file.allow=always",
            "-c",
            "safe.directory=*",
            "-C",
        ])
        .arg(&repo.git_dir)
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }
    let mut child = command
        .spawn()
        .map_err(|err| Fail::git(format!("failed to run git (is git installed?): {err}")))?;
    if let Some(bytes) = stdin {
        let mut pipe = child.stdin.take().expect("stdin");
        let payload = bytes.to_vec();
        let writer = std::thread::spawn(move || pipe.write_all(&payload));
        let output = child
            .wait_with_output()
            .map_err(|err| Fail::git(err.to_string()))?;
        let _ = writer.join();
        return Ok(GitOutput {
            ok: output.status.success(),
            stdout: output.stdout,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    let output = child
        .wait_with_output()
        .map_err(|err| Fail::git(err.to_string()))?;
    Ok(GitOutput {
        ok: output.status.success(),
        stdout: output.stdout,
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

pub(super) fn ensure_repo(repo: &Repo) -> Result<(), Fail> {
    if repo.git_dir.join("HEAD").is_file() {
        return Ok(());
    }
    fs::create_dir_all(&repo.git_dir).map_err(|err| Fail::git(err.to_string()))?;
    git_ok(
        repo,
        &["init", "--bare", "--initial-branch=main"],
        None,
        &[],
    )?;
    git_ok(repo, &["config", "gc.auto", "0"], None, &[])?;
    Ok(())
}

pub(super) fn local_head(repo: &Repo) -> Result<Option<String>, Fail> {
    match git_out(
        repo,
        &["rev-parse", "--verify", "--quiet", "refs/heads/main"],
        None,
        &[],
    )? {
        out if out.ok => {
            let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if is_hex(&sha, 40) {
                Ok(Some(sha))
            } else {
                Ok(None)
            }
        }
        _ => Ok(None),
    }
}

pub(super) fn is_ancestor(repo: &Repo, old: &str, new_sha: &str) -> Result<bool, Fail> {
    Ok(git_out(
        repo,
        &["merge-base", "--is-ancestor", old, new_sha],
        None,
        &[],
    )?
    .ok)
}
