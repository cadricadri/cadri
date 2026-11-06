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

/// Builds trees for a run of commits, each on top of the last, with one index and
/// as few `hash-object` calls as the caller allows (see `hash_texts`).
pub(super) struct TreeBuilder<'a> {
    repo: &'a Repo,
    work: Scratch,
    /// Blob SHA of every text hashed so far.
    hashed: HashMap<String, String>,
    /// The commit (`None` for an empty book) whose tree the index holds, if known.
    index_at: Option<Option<String>>,
}

impl<'a> TreeBuilder<'a> {
    pub(super) fn new(repo: &'a Repo) -> Self {
        TreeBuilder {
            repo,
            work: Scratch::new(&repo.scratch),
            hashed: HashMap::new(),
            index_at: None,
        }
    }

    /// Writes every text not hashed yet as a blob, with one `git hash-object`.
    pub(super) fn hash_texts<'t>(
        &mut self,
        texts: impl IntoIterator<Item = &'t str>,
    ) -> Result<(), Fail> {
        let mut todo: Vec<&str> = Vec::new();
        let mut seen = HashSet::new();
        for text in texts {
            if !self.hashed.contains_key(text) && seen.insert(text) {
                todo.push(text);
            }
        }
        if todo.is_empty() {
            return Ok(());
        }
        let mut list = String::new();
        for (index, text) in todo.iter().enumerate() {
            let file = self.work.dir.join(format!("blob-{index}"));
            fs::write(&file, text.as_bytes()).map_err(|err| Fail::git(err.to_string()))?;
            list.push_str(&file.to_string_lossy());
            list.push('\n');
        }
        let out = git_text(
            self.repo,
            &["hash-object", "-w", "--stdin-paths"],
            Some(list.as_bytes()),
            &[],
        )?;
        let shas: Vec<&str> = out.lines().map(str::trim).collect();
        if shas.len() != todo.len() || shas.iter().any(|sha| !is_hex(sha, 40)) {
            return Err(Fail::git("hash-object returned the wrong number of blobs"));
        }
        for (text, sha) in todo.into_iter().zip(shas) {
            self.hashed.insert(text.to_string(), sha.to_string());
        }
        Ok(())
    }

    /// The tree for `after` on top of `tip`. `after` holds only `Blob` entries that
    /// are in `before` or new `Text`. Call `committed` once the tree is committed.
    pub(super) fn build(
        &mut self,
        tip: Option<&str>,
        before: &BTreeMap<String, Source>,
        after: &BTreeMap<String, Source>,
    ) -> Result<Built, Fail> {
        self.hash_texts(after.values().filter_map(|source| match source {
            Source::Text(text) => Some(text.as_str()),
            Source::Blob(_) => None,
        }))?;
        let mut files = BTreeMap::new();
        let mut new_blobs = HashMap::new();
        let mut lines = String::new();
        for path in before.keys().filter(|path| !after.contains_key(*path)) {
            lines.push_str(&format!("0 {ZERO}\t{path}\n"));
        }
        for (path, source) in after {
            match source {
                Source::Blob(blob) => {
                    if before.get(path) != Some(source) {
                        lines.push_str(&format!("100644 {blob}\t{path}\n"));
                    }
                    files.insert(path.clone(), blob.clone());
                }
                Source::Text(text) => {
                    let sha = self.hashed[text].clone();
                    lines.push_str(&format!("100644 {sha}\t{path}\n"));
                    new_blobs.insert(sha.clone(), Arc::from(text.as_str()));
                    files.insert(path.clone(), sha);
                }
            }
        }
        let index = self.work.dir.join("index");
        let index = index.to_string_lossy();
        let env = [("GIT_INDEX_FILE", index.as_ref())];
        let wanted = tip.map(str::to_string);
        if self.index_at.as_ref() != Some(&wanted) {
            self.index_at = None;
            match tip {
                Some(tip) => git_ok(self.repo, &["read-tree", tip], None, &env)?,
                None => git_ok(self.repo, &["read-tree", "--empty"], None, &env)?,
            }
        }
        // From here until `committed`, the index matches no commit.
        self.index_at = None;
        git_ok(
            self.repo,
            &["update-index", "--index-info"],
            Some(lines.as_bytes()),
            &env,
        )?;
        let tree = git_text(self.repo, &["write-tree"], None, &env)?;
        Ok(Built {
            tree,
            files,
            new_blobs,
        })
    }

    /// The tree from the last `build` is now the tree of `commit`.
    pub(super) fn committed(&mut self, commit: &str) {
        self.index_at = Some(Some(commit.to_string()));
    }
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

/// Who a UI or API commit is written for. A robot is an agent acting for the
/// signed-in human, whose label it keeps.
pub(super) enum CommitAuthor<'a> {
    Human { user_id: &'a str, label: &'a str },
    Robot { agent: &'a str, label: &'a str },
}

pub(super) fn commit_tree(
    repo: &Repo,
    tree: &str,
    parent: Option<&str>,
    message: &str,
    author: &CommitAuthor,
) -> Result<String, Fail> {
    let mut args = vec!["commit-tree", tree];
    if let Some(parent) = parent {
        args.push("-p");
        args.push(parent);
    }
    let (kind, id, label, name, email) = match *author {
        CommitAuthor::Human { user_id, label } => (
            "human",
            user_id,
            label,
            sanitize_name(label),
            format!("{}@users.cadri.local", sanitize_email(user_id)),
        ),
        CommitAuthor::Robot { agent, label } => (
            "robot",
            agent,
            label,
            sanitize_name(agent),
            format!("{}@agents.cadri.local", sanitize_email(agent)),
        ),
    };
    let body = format!(
        "{message}\n\nCadri-Author-Kind: {kind}\nCadri-Author-Id: {id}\nCadri-Author-Label: {label}\n"
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

/// `^[A-Za-z0-9._-]{1,64}$`.
pub(super) fn valid_agent(agent: &str) -> bool {
    (1..=64).contains(&agent.len())
        && agent
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
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
