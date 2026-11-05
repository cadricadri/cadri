use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::http::StatusCode;

use crate::memory::types::{memory_path_ok, Change};

use super::error::Fail;
use super::git_cli_exec::{git_ok, git_text, Repo};
use super::ids::{is_hex, is_keep, MAX_FILE_BYTES, ZERO};
use super::snapshot::Snapshot;

/// Where a file's bytes come from in a commit being built: a blob already in the
/// repo, or new text that still has to be hashed.
#[derive(Clone, PartialEq, Eq)]
pub(super) enum Source {
    Blob(String),
    Text(String),
}

pub(super) const EMPTY_BLOB: &str = "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391";

/// The tip's files as sources, `.keep` markers included.
pub(super) fn current_sources(snapshot: &Snapshot) -> BTreeMap<String, Source> {
    let mut files: BTreeMap<String, Source> = snapshot
        .files
        .iter()
        .map(|(path, entry)| (path.clone(), Source::Blob(entry.blob.clone())))
        .collect();
    for directory in &snapshot.directories {
        files.insert(
            format!("{directory}/.keep"),
            Source::Blob(EMPTY_BLOB.to_string()),
        );
    }
    files
}

pub(super) fn apply_changes(
    files: &mut BTreeMap<String, Source>,
    changes: &[Change],
) -> Result<(), Fail> {
    let invalid = || Fail::api(StatusCode::BAD_REQUEST, "invalid memory path");
    for change in changes {
        match change {
            Change::Write { path, content } => {
                check_content(path, content)?;
                files.insert(path.clone(), Source::Text(content.clone()));
            }
            Change::Delete { path } => {
                if files.remove(path).is_none() {
                    return Err(invalid());
                }
            }
            Change::Rename { from, to } => {
                let Some(source) = files.remove(from) else {
                    return Err(invalid());
                };
                match &source {
                    Source::Text(content) => check_content(to, content)?,
                    Source::Blob(blob) => {
                        check_path(to)?;
                        if is_keep(to) && blob != EMPTY_BLOB {
                            return Err(invalid());
                        }
                    }
                }
                files.insert(to.clone(), source);
            }
        }
    }
    check_layout(files.keys().map(String::as_str))
}

pub(super) fn check_path(path: &str) -> Result<(), Fail> {
    if !memory_path_ok(path) {
        return Err(Fail::api(StatusCode::BAD_REQUEST, "invalid memory path"));
    }
    Ok(())
}

pub(super) fn check_content(path: &str, content: &str) -> Result<(), Fail> {
    check_path(path)?;
    if content.len() > MAX_FILE_BYTES {
        return Err(Fail::api(
            StatusCode::BAD_REQUEST,
            "memory file is too large",
        ));
    }
    if content.contains('\0') {
        return Err(Fail::api(
            StatusCode::BAD_REQUEST,
            "memory file must be utf-8 text",
        ));
    }
    if is_keep(path) && !content.is_empty() {
        return Err(Fail::api(StatusCode::BAD_REQUEST, "invalid memory path"));
    }
    Ok(())
}

/// Every path must be valid and no path may be a directory of another.
pub(super) fn check_layout<'a>(paths: impl IntoIterator<Item = &'a str>) -> Result<(), Fail> {
    let paths: Vec<&str> = paths.into_iter().collect();
    let all: HashSet<&str> = paths.iter().copied().collect();
    for path in &paths {
        if !memory_path_ok(path) {
            return Err(Fail::api(StatusCode::BAD_REQUEST, "invalid memory path"));
        }
        for (index, _) in path.match_indices('/') {
            if all.contains(&path[..index]) {
                return Err(Fail::api(StatusCode::BAD_REQUEST, "invalid memory path"));
            }
        }
    }
    Ok(())
}

pub(super) fn clean_message(message: &str) -> Result<String, Fail> {
    let kept: Vec<&str> = message
        .trim()
        .lines()
        .filter(|line| !line.starts_with("Cadri-Author-"))
        .collect();
    let body = kept.join("\n").trim().to_string();
    if body.is_empty() {
        return Ok("Update memory".to_string());
    }
    Ok(body)
}

pub(super) struct Built {
    pub tree: String,
    /// Every file of the new tree as `path -> blob sha`, `.keep` markers included.
    pub files: BTreeMap<String, String>,
    /// Text of the blobs this commit added.
    pub new_blobs: HashMap<String, Arc<str>>,
}

/// Builds the tree for `after` on top of `tip` with a fixed number of git calls.
/// `after` holds only `Blob` entries that are in `before` or new `Text`.
pub(super) fn build_tree(
    repo: &Repo,
    tip: Option<&str>,
    before: &BTreeMap<String, Source>,
    after: &BTreeMap<String, Source>,
) -> Result<Built, Fail> {
    let texts: Vec<(&String, &String)> = after
        .iter()
        .filter_map(|(path, source)| match source {
            Source::Text(text) => Some((path, text)),
            Source::Blob(_) => None,
        })
        .collect();
    let work = Scratch::new(&repo.scratch);
    let mut shas = Vec::new();
    if !texts.is_empty() {
        let mut list = String::new();
        for (index, (_, text)) in texts.iter().enumerate() {
            let file = work.dir.join(format!("blob-{index}"));
            fs::write(&file, text.as_bytes()).map_err(|err| Fail::git(err.to_string()))?;
            list.push_str(&file.to_string_lossy());
            list.push('\n');
        }
        let out = git_text(
            repo,
            &["hash-object", "-w", "--stdin-paths"],
            Some(list.as_bytes()),
            &[],
        )?;
        shas = out.lines().map(|line| line.trim().to_string()).collect();
        if shas.len() != texts.len() || shas.iter().any(|sha| !is_hex(sha, 40)) {
            return Err(Fail::git("hash-object returned the wrong number of blobs"));
        }
    }
    let mut files = BTreeMap::new();
    let mut new_blobs = HashMap::new();
    let mut lines = String::new();
    for path in before.keys().filter(|path| !after.contains_key(*path)) {
        lines.push_str(&format!("0 {ZERO}\t{path}\n"));
    }
    let mut next = shas.iter();
    for (path, source) in after {
        match source {
            Source::Blob(blob) => {
                if before.get(path) != Some(source) {
                    lines.push_str(&format!("100644 {blob}\t{path}\n"));
                }
                files.insert(path.clone(), blob.clone());
            }
            Source::Text(text) => {
                let sha = next.next().expect("one sha per text").clone();
                lines.push_str(&format!("100644 {sha}\t{path}\n"));
                new_blobs.insert(sha.clone(), Arc::from(text.as_str()));
                files.insert(path.clone(), sha);
            }
        }
    }
    let index = work.dir.join("index");
    let index = index.to_string_lossy();
    let env = [("GIT_INDEX_FILE", index.as_ref())];
    match tip {
        Some(tip) => git_ok(repo, &["read-tree", tip], None, &env)?,
        None => git_ok(repo, &["read-tree", "--empty"], None, &env)?,
    }
    git_ok(
        repo,
        &["update-index", "--index-info"],
        Some(lines.as_bytes()),
        &env,
    )?;
    let tree = git_text(repo, &["write-tree"], None, &env)?;
    Ok(Built {
        tree,
        files,
        new_blobs,
    })
}

/// A private directory under the repo's scratch, removed on drop.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(scratch: &Path) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = scratch.join(format!(
            "commit-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::create_dir_all(&dir);
        Scratch { dir }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

pub(super) fn commit_tree(
    repo: &Repo,
    tree: &str,
    parent: Option<&str>,
    message: &str,
    user_id: &str,
    label: &str,
) -> Result<String, Fail> {
    let mut args = vec!["commit-tree", tree];
    if let Some(parent) = parent {
        args.push("-p");
        args.push(parent);
    }
    let email = format!("{}@users.cadri.local", sanitize_email(user_id));
    let name = sanitize_name(label);
    let body = format!(
        "{message}\n\nCadri-Author-Kind: human\nCadri-Author-Id: {user_id}\nCadri-Author-Label: {label}\n"
    );
    git_text(
        repo,
        &args,
        Some(body.as_bytes()),
        &[
            ("GIT_AUTHOR_NAME", name.as_str()),
            ("GIT_AUTHOR_EMAIL", email.as_str()),
            ("GIT_COMMITTER_NAME", "Cadri Engine"),
            ("GIT_COMMITTER_EMAIL", "engine@cadri.local"),
        ],
    )
}

pub(super) fn sanitize_name(label: &str) -> String {
    let name: String = label
        .chars()
        .filter(|ch| *ch != '<' && *ch != '>' && *ch != '\n' && *ch != '\r')
        .collect();
    if name.trim().is_empty() {
        "Cadri".to_string()
    } else {
        name
    }
}

pub(super) fn sanitize_email(user_id: &str) -> String {
    let email: String = user_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if email.is_empty() {
        "user".to_string()
    } else {
        email
    }
}
