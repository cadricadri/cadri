use std::collections::HashMap;

use axum::http::StatusCode;

use crate::memory::types::{Author, BlameEntry, MemoryCommit};
#[cfg(test)]
use crate::memory::types::{MemoryFile, MemoryStore};

use super::author::{author_from, display_message};
use super::error::Fail;
use super::git_cli_exec::{git_bytes, git_text, local_head, Repo};
use super::ids::is_hex;
use super::ids::is_keep;
use super::tree::{list_tree, parse_blame};

pub(super) fn blame_file(repo: &Repo, path: &str) -> Result<Vec<BlameEntry>, Fail> {
    if is_keep(path) {
        return Err(Fail::api(StatusCode::NOT_FOUND, "memory file not found"));
    }
    let entries = list_tree(repo, "HEAD")?;
    let Some((mode, sha, _)) = entries.into_iter().find(|(_, _, entry)| entry == path) else {
        return Err(Fail::api(StatusCode::NOT_FOUND, "memory file not found"));
    };
    if mode != "100644" && mode != "100755" {
        return Err(Fail::api(StatusCode::BAD_REQUEST, "invalid memory path"));
    }
    let bytes = git_bytes(repo, &["cat-file", "blob", &sha], None, &[])?;
    let content =
        String::from_utf8(bytes).map_err(|_| Fail::git(format!("{path} is not utf-8")))?;
    let mut authors = HashMap::new();
    line_blame(repo, path, &content, &mut authors)
}

fn line_blame(
    repo: &Repo,
    path: &str,
    content: &str,
    authors: &mut HashMap<String, Author>,
) -> Result<Vec<BlameEntry>, Fail> {
    let mut blamed = parse_blame(&git_text(
        repo,
        &["blame", "-p", "HEAD", "--", path],
        None,
        &[],
    )?);
    if blamed.is_empty() && content.is_empty() {
        if let Some(head) = local_head(repo)? {
            blamed.push(head);
        }
    }
    // `split('\n')` counts a trailing newline as its own line. Git blame does not.
    let expected = content.split('\n').count();
    if blamed.len() + 1 == expected && content.ends_with('\n') {
        if let Some(last) = blamed.last().cloned() {
            blamed.push(last);
        }
    }
    if blamed.len() != expected {
        return Err(Fail::git(format!(
            "blame for {path} does not match the file"
        )));
    }
    Ok(blamed
        .into_iter()
        .map(|commit_id| {
            let author = author_of(repo, &commit_id, authors);
            BlameEntry { author, commit_id }
        })
        .collect())
}

/// Previous page-load view: full history, per-file `cat-file`, and blame for every file.
/// Kept so the read benchmark can time this path against `view_tree`.
#[cfg(test)]
pub(super) fn view_legacy(repo: &Repo) -> Result<MemoryStore, Fail> {
    let Some(_) = local_head(repo)? else {
        return Ok(MemoryStore::empty());
    };
    let commits = read_commits(repo)?;
    let mut authors: HashMap<String, Author> = commits
        .iter()
        .map(|commit| (commit.id.clone(), commit.author.clone()))
        .collect();
    let mut files = Vec::new();
    let mut directories = Vec::new();
    for (mode, sha, path) in list_tree(repo, "HEAD")? {
        if mode != "100644" && mode != "100755" {
            return Err(Fail::api(StatusCode::BAD_REQUEST, "invalid memory path"));
        }
        if is_keep(&path) {
            if let Some((directory, _)) = path.rsplit_once('/') {
                directories.push(directory.to_string());
            }
            continue;
        }
        let bytes = git_bytes(repo, &["cat-file", "blob", &sha], None, &[])?;
        let content =
            String::from_utf8(bytes).map_err(|_| Fail::git(format!("{path} is not utf-8")))?;
        let blame = line_blame(repo, &path, &content, &mut authors)?;
        files.push(MemoryFile {
            path,
            content,
            blame,
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    directories.sort();
    directories.dedup();
    Ok(MemoryStore {
        files,
        directories,
        commits,
    })
}

pub(super) fn author_of(repo: &Repo, sha: &str, cache: &mut HashMap<String, Author>) -> Author {
    if let Some(author) = cache.get(sha) {
        return author.clone();
    }
    let message = git_text(repo, &["log", "-1", "--format=%B", sha], None, &[]).unwrap_or_default();
    let author = author_from(&message);
    cache.insert(sha.to_string(), author.clone());
    author
}

pub(super) fn read_commits(repo: &Repo) -> Result<Vec<MemoryCommit>, Fail> {
    let text = git_text(
        repo,
        &[
            "log",
            "--format=format:%x02%H%x1e%cI%x1e%B%x1e",
            "--name-only",
        ],
        None,
        &[],
    )?;
    let mut commits = Vec::new();
    for record in text.split('\u{2}') {
        if record.trim().is_empty() {
            continue;
        }
        let mut parts = record.split('\u{1e}');
        let sha = parts.next().unwrap_or("").trim();
        let at = parts.next().unwrap_or("").trim();
        let message = parts.next().unwrap_or("");
        let names = parts.next().unwrap_or("");
        if !is_hex(sha, 40) {
            continue;
        }
        let files = names
            .lines()
            .map(str::trim)
            .filter(|path| !path.is_empty() && !is_keep(path))
            .map(str::to_string)
            .collect();
        commits.push(MemoryCommit {
            id: sha.to_string(),
            at: at.to_string(),
            author: author_from(message),
            message: display_message(message),
            files,
        });
    }
    Ok(commits)
}
