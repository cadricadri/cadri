use std::collections::BTreeMap;

use axum::http::StatusCode;

use crate::memory::types::{memory_path_ok, Change};

use super::error::Fail;
use super::git_cli_exec::{git_bytes, git_text, local_head, Repo};
use super::ids::{is_keep, MAX_FILE_BYTES};
use super::tree::list_tree;

pub(super) fn current_files(repo: &Repo) -> Result<BTreeMap<String, String>, Fail> {
    if local_head(repo)?.is_none() {
        return Ok(BTreeMap::new());
    }
    let mut files = BTreeMap::new();
    for (mode, sha, path) in list_tree(repo, "HEAD")? {
        if mode != "100644" && mode != "100755" {
            return Err(Fail::api(StatusCode::BAD_REQUEST, "invalid memory path"));
        }
        let bytes = git_bytes(repo, &["cat-file", "blob", &sha], None, &[])?;
        let content =
            String::from_utf8(bytes).map_err(|_| Fail::git("memory blob is not utf-8"))?;
        files.insert(path, content);
    }
    Ok(files)
}

pub(super) fn apply_changes(
    files: &mut BTreeMap<String, String>,
    changes: &[Change],
) -> Result<(), Fail> {
    for change in changes {
        match change {
            Change::Write { path, content } => {
                check_content(path, content)?;
                files.insert(path.clone(), content.clone());
            }
            Change::Delete { path } => {
                if files.remove(path).is_none() {
                    return Err(Fail::api(StatusCode::BAD_REQUEST, "invalid memory path"));
                }
            }
            Change::Rename { from, to } => {
                let Some(content) = files.remove(from) else {
                    return Err(Fail::api(StatusCode::BAD_REQUEST, "invalid memory path"));
                };
                check_content(to, &content)?;
                files.insert(to.clone(), content);
            }
        }
    }
    check_layout(files)
}

pub(super) fn check_content(path: &str, content: &str) -> Result<(), Fail> {
    if !memory_path_ok(path) || (is_keep(path) && path == ".keep") {
        return Err(Fail::api(StatusCode::BAD_REQUEST, "invalid memory path"));
    }
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

pub(super) fn check_layout(files: &BTreeMap<String, String>) -> Result<(), Fail> {
    let paths: Vec<&str> = files.keys().map(String::as_str).collect();
    for (index, path) in paths.iter().enumerate() {
        if !memory_path_ok(path) {
            return Err(Fail::api(StatusCode::BAD_REQUEST, "invalid memory path"));
        }
        for other in paths.iter().skip(index + 1) {
            if other.starts_with(&format!("{path}/")) || path.starts_with(&format!("{other}/")) {
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

pub(super) fn write_tree(repo: &Repo, files: &BTreeMap<String, String>) -> Result<String, Fail> {
    fn insert(dir: &mut Dir, path: &str, content: String) {
        if let Some((head, tail)) = path.split_once('/') {
            insert(dir.dirs.entry(head.to_string()).or_default(), tail, content);
        } else {
            dir.files.insert(path.to_string(), content);
        }
    }
    let mut root = Dir::default();
    for (path, content) in files {
        insert(&mut root, path, content.clone());
    }
    write_dir(repo, &root)
}

#[derive(Default)]
pub(super) struct Dir {
    files: BTreeMap<String, String>,
    dirs: BTreeMap<String, Dir>,
}

pub(super) fn write_dir(repo: &Repo, dir: &Dir) -> Result<String, Fail> {
    let mut entries = Vec::new();
    for (name, content) in &dir.files {
        let sha = hash_object(repo, content.as_bytes())?;
        entries.push((name.clone(), format!("100644 blob {sha}\t{name}\n")));
    }
    for (name, child) in &dir.dirs {
        let sha = write_dir(repo, child)?;
        entries.push((format!("{name}/"), format!("040000 tree {sha}\t{name}\n")));
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let script: String = entries.into_iter().map(|(_, line)| line).collect();
    git_text(repo, &["mktree"], Some(script.as_bytes()), &[])
}

pub(super) fn hash_object(repo: &Repo, bytes: &[u8]) -> Result<String, Fail> {
    git_text(repo, &["hash-object", "-w", "--stdin"], Some(bytes), &[])
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
