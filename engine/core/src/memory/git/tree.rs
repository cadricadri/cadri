use std::collections::HashMap;

use super::error::Fail;
use super::git_cli_exec::{git_bytes, Repo};
use super::ids::is_hex;

pub(super) fn list_tree(repo: &Repo, rev: &str) -> Result<Vec<(String, String, String)>, Fail> {
    let bytes = git_bytes(repo, &["ls-tree", "-r", "-z", rev], None, &[])?;
    let mut rest = bytes.as_slice();
    let mut entries = Vec::new();
    while !rest.is_empty() {
        let Some(end) = rest.iter().position(|byte| *byte == 0) else {
            return Err(Fail::git("truncated ls-tree"));
        };
        let entry = std::str::from_utf8(&rest[..end]).map_err(|_| Fail::git("ls-tree path"))?;
        rest = &rest[end + 1..];
        let Some((meta, path)) = entry.split_once('\t') else {
            return Err(Fail::git("ls-tree entry"));
        };
        let mut parts = meta.split(' ');
        let mode = parts.next().unwrap_or("");
        let _kind = parts.next();
        let sha = parts.next().unwrap_or("");
        entries.push((mode.to_string(), sha.to_string(), path.to_string()));
    }
    Ok(entries)
}

/// One `git cat-file --batch` for every blob. Callers share a blob sha across paths.
pub(super) fn cat_blobs(repo: &Repo, shas: &[String]) -> Result<HashMap<String, Vec<u8>>, Fail> {
    let mut unique = Vec::new();
    for sha in shas {
        if !unique.iter().any(|seen: &String| seen == sha) {
            unique.push(sha.clone());
        }
    }
    if unique.is_empty() {
        return Ok(HashMap::new());
    }
    let mut stdin = Vec::new();
    for sha in &unique {
        stdin.extend(sha.as_bytes());
        stdin.push(b'\n');
    }
    let bytes = git_bytes(repo, &["cat-file", "--batch"], Some(&stdin), &[])?;
    let parsed = parse_cat_batch(&bytes)?;
    if parsed.len() != unique.len() {
        return Err(Fail::git(
            "cat-file --batch returned the wrong number of blobs",
        ));
    }
    let mut out = HashMap::new();
    for (sha, body) in parsed {
        out.insert(sha, body);
    }
    Ok(out)
}

/// The pre-change blob read: one jailed `git cat-file` process per blob.
#[cfg(test)]
pub(super) fn cat_blobs_serial(
    repo: &Repo,
    shas: &[String],
) -> Result<HashMap<String, Vec<u8>>, Fail> {
    let mut out = HashMap::new();
    for sha in shas {
        if out.contains_key(sha) {
            continue;
        }
        let bytes = git_bytes(repo, &["cat-file", "blob", sha], None, &[])?;
        out.insert(sha.clone(), bytes);
    }
    Ok(out)
}

pub(super) fn parse_cat_batch(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, Fail> {
    let mut rest = bytes;
    let mut out = Vec::new();
    while !rest.is_empty() {
        let Some(end) = rest.iter().position(|byte| *byte == b'\n') else {
            return Err(Fail::git("truncated cat-file header"));
        };
        let header = std::str::from_utf8(&rest[..end]).map_err(|_| Fail::git("cat-file header"))?;
        rest = &rest[end + 1..];
        if header.ends_with(" missing") {
            return Err(Fail::git(format!("cat-file --batch {header}")));
        }
        let mut parts = header.split(' ');
        let sha = parts.next().unwrap_or("");
        let kind = parts.next().unwrap_or("");
        let size: usize = parts
            .next()
            .unwrap_or("")
            .parse()
            .map_err(|_| Fail::git("cat-file size"))?;
        if kind != "blob" || !is_hex(sha, 40) {
            return Err(Fail::git(format!("cat-file --batch {header}")));
        }
        if rest.len() < size + 1 || rest[size] != b'\n' {
            return Err(Fail::git("truncated cat-file blob"));
        }
        out.push((sha.to_string(), rest[..size].to_vec()));
        rest = &rest[size + 1..];
    }
    Ok(out)
}

pub(super) fn parse_blame(text: &str) -> Vec<String> {
    let mut shas = Vec::new();
    let mut lines = text.lines();
    while let Some(header) = lines.next() {
        let Some(sha) = header.split_whitespace().next() else {
            continue;
        };
        if !is_hex(sha, 40) {
            continue;
        }
        for line in lines.by_ref() {
            if line.starts_with('\t') {
                shas.push(sha.to_string());
                break;
            }
        }
    }
    shas
}
