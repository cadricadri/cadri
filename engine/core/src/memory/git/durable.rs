//! Durable store interface shared by the directory store and S3.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::error::{s3_fail, Fail};
use crate::memory::s3::S3Books;

pub(super) enum Created {
    Stored,
    Exists,
}

pub(super) trait Durable: Send + Sync {
    /// `key` is relative to `repos/<repo_id>/`, e.g. "log/00000000000000000001".
    fn get(&self, repo_id: &str, key: &str) -> Result<Option<Vec<u8>>, Fail>;
    /// Atomic create-if-absent. Never overwrites.
    fn create(&self, repo_id: &str, key: &str, bytes: &[u8]) -> Result<Created, Fail>;
    /// Keys under `prefix` (relative, e.g. "log/"), sorted ascending, all pages,
    /// strictly greater than `start_after` when given.
    fn list(
        &self,
        repo_id: &str,
        prefix: &str,
        start_after: Option<&str>,
    ) -> Result<Vec<String>, Fail>;
}

impl<T: Durable + ?Sized> Durable for Arc<T> {
    fn get(&self, repo_id: &str, key: &str) -> Result<Option<Vec<u8>>, Fail> {
        (**self).get(repo_id, key)
    }

    fn create(&self, repo_id: &str, key: &str, bytes: &[u8]) -> Result<Created, Fail> {
        (**self).create(repo_id, key, bytes)
    }

    fn list(
        &self,
        repo_id: &str,
        prefix: &str,
        start_after: Option<&str>,
    ) -> Result<Vec<String>, Fail> {
        (**self).list(repo_id, prefix, start_after)
    }
}

pub(super) fn log_key(seq: u64) -> String {
    format!("log/{seq:020}")
}

pub(super) fn checkpoint_key(seq: u64) -> String {
    format!("checkpoint/{seq:020}")
}

/// Parses the 20 digits after the last '/'.
pub(super) fn seq_of(key: &str) -> Option<u64> {
    let name = key.rsplit('/').next()?;
    if name.len() == 20 && name.bytes().all(|byte| byte.is_ascii_digit()) {
        name.parse().ok()
    } else {
        None
    }
}

pub(super) struct DirDurable {
    pub root: PathBuf,
}

impl DirDurable {
    pub(super) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn repo_dir(&self, repo_id: &str) -> Result<PathBuf, Fail> {
        check_part(repo_id)?;
        Ok(self.root.join("repos").join(repo_id))
    }

    fn file(&self, repo_id: &str, key: &str) -> Result<PathBuf, Fail> {
        check_part(key)?;
        Ok(self.repo_dir(repo_id)?.join(key))
    }
}

fn check_part(part: &str) -> Result<(), Fail> {
    let bad = part.is_empty()
        || part.starts_with('/')
        || part.contains('\\')
        || part.split('/').any(|piece| piece == "..");
    if bad {
        return Err(Fail::git(format!("bad durable key {part:?}")));
    }
    Ok(())
}

fn io(err: std::io::Error) -> Fail {
    Fail::git(err.to_string())
}

impl Durable for DirDurable {
    fn get(&self, repo_id: &str, key: &str) -> Result<Option<Vec<u8>>, Fail> {
        match fs::read(self.file(repo_id, key)?) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(io(err)),
        }
    }

    fn create(&self, repo_id: &str, key: &str, bytes: &[u8]) -> Result<Created, Fail> {
        static TEMPS: AtomicU64 = AtomicU64::new(0);
        let path = self.file(repo_id, key)?;
        let dir = path.parent().ok_or_else(|| Fail::git("durable key path"))?;
        fs::create_dir_all(dir).map_err(io)?;
        let tmp = dir.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            TEMPS.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&tmp, bytes).map_err(io)?;
        let linked = fs::hard_link(&tmp, &path);
        let _ = fs::remove_file(&tmp);
        match linked {
            Ok(()) => Ok(Created::Stored),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(Created::Exists),
            Err(err) => Err(io(err)),
        }
    }

    fn list(
        &self,
        repo_id: &str,
        prefix: &str,
        start_after: Option<&str>,
    ) -> Result<Vec<String>, Fail> {
        let (dir_part, name_prefix) = match prefix.rfind('/') {
            Some(at) => (&prefix[..at + 1], &prefix[at + 1..]),
            None => ("", prefix),
        };
        let mut dir = self.repo_dir(repo_id)?;
        if !dir_part.is_empty() {
            check_part(dir_part)?;
            dir = dir.join(dir_part);
        }
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(io(err)),
        };
        let mut keys = Vec::new();
        for entry in entries {
            let entry = entry.map_err(io)?;
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if name.starts_with(".tmp-") || !name.starts_with(name_prefix) {
                continue;
            }
            if entry.file_type().map_err(io)?.is_file() {
                keys.push(format!("{dir_part}{name}"));
            }
        }
        keys.retain(|key| start_after.is_none_or(|after| key.as_str() > after));
        keys.sort();
        Ok(keys)
    }
}

impl Durable for S3Books {
    fn get(&self, repo_id: &str, key: &str) -> Result<Option<Vec<u8>>, Fail> {
        self.get_key(repo_id, key).map_err(s3_fail)
    }

    fn create(&self, repo_id: &str, key: &str, bytes: &[u8]) -> Result<Created, Fail> {
        match self.create_key(repo_id, key, bytes).map_err(s3_fail)? {
            true => Ok(Created::Stored),
            false => Ok(Created::Exists),
        }
    }

    fn list(
        &self,
        repo_id: &str,
        prefix: &str,
        start_after: Option<&str>,
    ) -> Result<Vec<String>, Fail> {
        self.list_keys(repo_id, prefix, start_after)
            .map_err(s3_fail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::s3::FakeS3;

    const REPO: &str = "abcdef0123456789abcdef0123456789";

    fn temp_root() -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "cadri-durable-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn stores() -> Vec<(&'static str, Arc<dyn Durable>, Option<FakeS3>)> {
        let fake = FakeS3::start();
        let s3 = S3Books::from_client("memories", S3Books::test_client(&fake.endpoint));
        vec![
            ("directory", Arc::new(DirDurable::new(temp_root())), None),
            ("s3", Arc::new(s3), Some(fake)),
        ]
    }

    fn stored(created: Created) -> bool {
        matches!(created, Created::Stored)
    }

    #[test]
    fn create_get_and_list_behave_the_same_on_both_stores() {
        for (name, store, _fake) in stores() {
            let store: &dyn Durable = &*store;
            assert!(store.get(REPO, "log/missing").unwrap().is_none(), "{name}");
            assert!(
                stored(store.create(REPO, "log/a", b"first").unwrap()),
                "{name}"
            );
            assert!(
                !stored(store.create(REPO, "log/a", b"second").unwrap()),
                "{name}"
            );
            assert_eq!(
                store.get(REPO, "log/a").unwrap().unwrap(),
                b"first",
                "{name}"
            );
            assert_eq!(store.list(REPO, "log/", None).unwrap(), ["log/a"], "{name}");

            for seq in [3, 1, 2] {
                store.create(REPO, &log_key(seq), b"x").unwrap();
            }
            store.create(REPO, &checkpoint_key(0), b"c").unwrap();
            let all = store.list(REPO, "log/", None).unwrap();
            assert_eq!(
                all,
                [
                    "log/00000000000000000001",
                    "log/00000000000000000002",
                    "log/00000000000000000003",
                    "log/a"
                ],
                "{name}"
            );
            let after = store.list(REPO, "log/", Some(&log_key(1))).unwrap();
            assert_eq!(after.len(), 3, "{name}");
            assert_eq!(after[0], log_key(2), "{name}");
            assert_eq!(
                store.list(REPO, "checkpoint/", None).unwrap(),
                [checkpoint_key(0)],
                "{name}"
            );
            assert!(
                store.list(REPO, "nothing/", None).unwrap().is_empty(),
                "{name}"
            );
            let other = "fedcba9876543210fedcba9876543210";
            assert!(
                store.list(other, "log/", None).unwrap().is_empty(),
                "{name}"
            );
        }
    }

    #[test]
    fn list_returns_more_than_one_page_of_keys() {
        for (name, store, _fake) in stores() {
            let store: &dyn Durable = &*store;
            for seq in 1..=1005 {
                store.create(REPO, &log_key(seq), b"x").unwrap();
            }
            let keys = store.list(REPO, "log/", None).unwrap();
            assert_eq!(keys.len(), 1005, "{name}");
            assert_eq!(keys[0], log_key(1), "{name}");
            assert_eq!(keys[1004], log_key(1005), "{name}");
            assert!(keys.windows(2).all(|pair| pair[0] < pair[1]), "{name}");
            let tail = store.list(REPO, "log/", Some(&log_key(1001))).unwrap();
            assert_eq!(tail.len(), 4, "{name}");
        }
    }

    #[test]
    fn eight_threads_creating_one_key_store_it_once() {
        for (name, store, _fake) in stores() {
            let handles: Vec<_> = (0..8)
                .map(|n| {
                    let store = store.clone();
                    std::thread::spawn(move || {
                        let body = format!("writer {n}");
                        let created = store.create(REPO, &log_key(1), body.as_bytes()).unwrap();
                        (body, stored(created))
                    })
                })
                .collect();
            let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
            let winners: Vec<_> = results.iter().filter(|(_, won)| *won).collect();
            assert_eq!(winners.len(), 1, "{name}");
            let kept = store.get(REPO, &log_key(1)).unwrap().unwrap();
            assert_eq!(kept, winners[0].0.as_bytes(), "{name}");
            assert_eq!(store.list(REPO, "log/", None).unwrap().len(), 1, "{name}");
        }
    }

    #[test]
    fn directory_store_rejects_keys_that_leave_the_repo() {
        let store = DirDurable::new(temp_root());
        assert!(store.get(REPO, "../other/log/1").is_err());
        assert!(store.create(REPO, "/etc/x", b"x").is_err());
    }

    #[test]
    fn seq_of_reads_the_padded_number() {
        assert_eq!(seq_of(&log_key(7)), Some(7));
        assert_eq!(seq_of(&checkpoint_key(0)), Some(0));
        assert_eq!(seq_of("log/7"), None);
        assert_eq!(seq_of("log/a"), None);
        assert_eq!(log_key(1), "log/00000000000000000001");
    }
}
