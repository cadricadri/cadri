use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use axum::http::StatusCode;

use crate::memory::types::{MemoryFile, MemoryStore, MemoryTree, OpenFile, TreeEntry};

use super::error::Fail;
use super::git_cli_exec::Repo;
use super::ids::is_keep;
use super::tree::{cat_blobs, list_tree};

pub(super) struct FileEntry {
    pub blob: String,
    pub size: u64,
}

/// A book's tip held in memory: paths, blob SHAs, and file text.
pub(super) struct Snapshot {
    pub tip: Option<String>,
    pub seq: u64,
    pub files: BTreeMap<String, FileEntry>,
    pub directories: Vec<String>,
    pub blobs: HashMap<String, Arc<str>>,
}

impl Snapshot {
    /// Paths, blob SHAs, and sizes, plus the text of one file: `open` if it exists,
    /// else `README.md`, else the first file.
    pub(super) fn to_tree(&self, open: Option<&str>) -> MemoryTree {
        let chosen = open
            .filter(|path| self.files.contains_key(*path))
            .or_else(|| self.files.contains_key("README.md").then_some("README.md"))
            .or_else(|| self.files.keys().next().map(String::as_str));
        let open = chosen.and_then(|path| {
            let entry = self.files.get(path)?;
            let content = self.blobs.get(&entry.blob)?;
            Some(OpenFile {
                path: path.to_string(),
                blob: entry.blob.clone(),
                content: content.to_string(),
            })
        });
        MemoryTree {
            tip: self.tip.clone(),
            seq: self.seq,
            files: self
                .files
                .iter()
                .map(|(path, entry)| TreeEntry {
                    path: path.clone(),
                    blob: entry.blob.clone(),
                    size: entry.size,
                })
                .collect(),
            directories: self.directories.clone(),
            open,
        }
    }

    /// Legacy view: file text, no blame, no history.
    pub(super) fn to_store(&self) -> MemoryStore {
        let files = self
            .files
            .iter()
            .map(|(path, entry)| MemoryFile {
                path: path.clone(),
                content: self
                    .blobs
                    .get(&entry.blob)
                    .map(|text| text.to_string())
                    .unwrap_or_default(),
                blame: Vec::new(),
            })
            .collect();
        MemoryStore {
            files,
            directories: self.directories.clone(),
            commits: Vec::new(),
        }
    }
}

/// Reads `tip` from the repo. Blobs already in `previous` are reused, so only new
/// file text is read from git.
pub(super) fn snapshot_from_repo(
    repo: &Repo,
    tip: Option<&str>,
    seq: u64,
    previous: Option<&Snapshot>,
) -> Result<Snapshot, Fail> {
    let Some(tip) = tip else {
        return Ok(Snapshot {
            tip: None,
            seq,
            files: BTreeMap::new(),
            directories: Vec::new(),
            blobs: HashMap::new(),
        });
    };
    let mut entries = Vec::new();
    let mut directories = Vec::new();
    for (mode, sha, path) in list_tree(repo, tip)? {
        if mode != "100644" && mode != "100755" {
            return Err(Fail::api(StatusCode::BAD_REQUEST, "invalid memory path"));
        }
        if is_keep(&path) {
            if let Some((directory, _)) = path.rsplit_once('/') {
                directories.push(directory.to_string());
            }
            continue;
        }
        entries.push((sha, path));
    }
    let mut blobs: HashMap<String, Arc<str>> = HashMap::new();
    let mut missing = Vec::new();
    for (sha, _) in &entries {
        if blobs.contains_key(sha) {
            continue;
        }
        match previous.and_then(|previous| previous.blobs.get(sha)) {
            Some(text) => {
                blobs.insert(sha.clone(), Arc::clone(text));
            }
            None => missing.push(sha.clone()),
        }
    }
    let mut bodies = cat_blobs(repo, &missing)?;
    let mut files = BTreeMap::new();
    for (sha, path) in entries {
        if !blobs.contains_key(&sha) {
            let bytes = bodies
                .remove(&sha)
                .ok_or_else(|| Fail::git(format!("blob {sha} missing from cat-file --batch")))?;
            let text =
                String::from_utf8(bytes).map_err(|_| Fail::git(format!("{path} is not utf-8")))?;
            blobs.insert(sha.clone(), Arc::from(text));
        }
        let size = blobs[&sha].len() as u64;
        files.insert(path, FileEntry { blob: sha, size });
    }
    directories.sort();
    directories.dedup();
    Ok(Snapshot {
        tip: Some(tip.to_string()),
        seq,
        files,
        directories,
        blobs,
    })
}
