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

