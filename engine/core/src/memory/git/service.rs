use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::StatusCode;

use crate::memory::s3::S3Books;
use crate::memory::types::{Change, MemoryStore};
use crate::ApiError;

use super::commit::{apply_changes, clean_message, commit_tree, current_files, write_tree};
use super::error::{into_api, s3_fail, Fail};
use super::git_cli_exec::{
    ensure_repo, git_bytes, git_ok, is_ancestor, local_head, object_exists, Repo,
};
use super::ids::{is_hex, valid_repo_id, ZERO};
use super::protocol::{
    advertise_receive, advertise_upload, index_pack, inspect, ready, report, split_pack,
    unpack_fail, GitHttp, RECEIVE_AD, UPLOAD_AD, UPLOAD_RESULT,
};
use super::store::{cas_ref_file, clear_stage, obj_path, put_bundle_file, read_sha_file, ref_path};
use super::view::view;

pub(super) enum Backend {
    Directory(PathBuf),
    S3(Box<S3Books>),
}

struct Inner {
    repos: PathBuf,
    backend: Backend,
    scratch: PathBuf,
    mount: Option<PathBuf>,
    jail: bool,
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    cache: Mutex<HashMap<String, (String, MemoryStore)>>,
    /// Test-only rendezvous so two servers reach the compare-and-swap together.
    gate: Mutex<Option<Arc<PublishGate>>>,
    /// Test-only. The next this many compare-and-swaps fail, so a UI commit can
    /// be shown exhausting its retries.
    cas_failures: Mutex<u32>,
}

#[derive(Clone)]
pub struct MemoryService {
    inner: Arc<Inner>,
}

impl MemoryService {
    pub fn open_local(root: impl Into<PathBuf>, mount: Option<PathBuf>, jail: bool) -> Self {
        let root = root.into();
        Self::new(
            root.join("repos"),
            Backend::Directory(root.join("store")),
            root.join("scratch"),
            mount,
            jail,
        )
    }

    pub fn open_s3(
        root: impl Into<PathBuf>,
        mount: Option<PathBuf>,
        jail: bool,
        books: S3Books,
    ) -> Self {
        let root = root.into();
        Self::new(
            root.join("repos"),
            Backend::S3(Box::new(books)),
            root.join("scratch"),
            mount,
            jail,
        )
    }

    pub(super) fn new(
        repos: PathBuf,
        backend: Backend,
        scratch: PathBuf,
        mount: Option<PathBuf>,
        jail: bool,
    ) -> Self {
        MemoryService {
            inner: Arc::new(Inner {
                repos,
                backend,
                scratch,
                mount,
                jail,
                locks: Mutex::new(HashMap::new()),
                cache: Mutex::new(HashMap::new()),
                gate: Mutex::new(None),
                cas_failures: Mutex::new(0),
            }),
        }
    }

    /// Test helper: the next `times` compare-and-swaps fail.
    pub fn fail_next_cas(&self, times: u32) {
        *self.inner.cas_failures.lock().expect("cas failures") = times;
    }

    /// Test helper: directory store with explicit git/scratch paths.
    pub fn open_at(repos: PathBuf, store: PathBuf, scratch: PathBuf) -> Self {
        Self::new(repos, Backend::Directory(store), scratch, None, false)
    }

    /// Test helper: two servers wait in `publish` until `parties` arrive, then
    /// exactly one compare-and-swap proceeds.
    pub fn arm_publish_gate(&self, gate: Arc<PublishGate>) {
        *self.inner.gate.lock().expect("publish gate") = Some(gate);
    }

    pub(crate) fn read_store(&self, repo_id: &str) -> Result<MemoryStore, ApiError> {
        self.with_repo(repo_id, |repo| {
            self.sync(repo, repo_id)?;
            self.cached_view(repo, repo_id)
        })
        .map_err(into_api)
    }

    pub(crate) fn commit(
        &self,
        repo_id: &str,
        user_id: &str,
        label: &str,
        message: &str,
        changes: &[Change],
    ) -> Result<MemoryStore, ApiError> {
        for _ in 0..8 {
            match self.with_repo(repo_id, |repo| {
                self.commit_once(repo, repo_id, user_id, label, message, changes)
            }) {
                Ok(store) => return Ok(store),
                Err(Fail::Conflict) => continue,
                Err(Fail::Api(err)) => return Err(err),
            }
        }
        Err(ApiError::new(StatusCode::CONFLICT, "memory book changed"))
    }

    pub(crate) fn serve_git(
        &self,
        repo_id: &str,
        path: &str,
        is_post: bool,
        service: Option<&str>,
        body: &[u8],
        user_id: &str,
    ) -> Result<GitHttp, ApiError> {
        self.with_repo(repo_id, |repo| {
            self.git_once(repo, repo_id, path, is_post, service, body, user_id)
        })
        .map_err(into_api)
    }

    fn with_repo<T>(
        &self,
        repo_id: &str,
        f: impl FnOnce(&Repo) -> Result<T, Fail>,
    ) -> Result<T, Fail> {
        if !valid_repo_id(repo_id) {
            return Err(Fail::api(
                StatusCode::INTERNAL_SERVER_ERROR,
                "invalid memory repo",
            ));
        }
        let lock = {
            let mut locks = self.inner.locks.lock().map_err(|_| {
                Fail::api(StatusCode::INTERNAL_SERVER_ERROR, "memory lock poisoned")
            })?;
            locks
                .entry(repo_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = lock
            .lock()
            .map_err(|_| Fail::api(StatusCode::INTERNAL_SERVER_ERROR, "memory lock poisoned"))?;
        let git_dir = self.inner.repos.join(format!("{repo_id}.git"));
        let scratch = self.inner.scratch.join(repo_id);
        fs::create_dir_all(&scratch).map_err(|err| Fail::git(err.to_string()))?;
        let repo = Repo {
            git_dir,
            scratch,
            jail: self.inner.jail,
        };
        ensure_repo(&repo)?;
        f(&repo)
    }

    fn commit_once(
        &self,
        repo: &Repo,
        repo_id: &str,
        user_id: &str,
        label: &str,
        message: &str,
        changes: &[Change],
    ) -> Result<MemoryStore, Fail> {
        self.sync(repo, repo_id)?;
        let message = clean_message(message)?;
        let mut files = current_files(repo)?;
        let before = files.clone();
        apply_changes(&mut files, changes)?;
        if files == before {
            return self.cached_view(repo, repo_id);
        }
        let tree = write_tree(repo, &files)?;
        let parent = local_head(repo)?;
        let sha = commit_tree(repo, &tree, parent.as_deref(), &message, user_id, label)?;
        self.publish(repo, repo_id, parent.as_deref(), &sha)?;
        self.cached_view(repo, repo_id)
    }

    #[allow(clippy::too_many_arguments)] // path, verb, service, and body are one git HTTP call
    fn git_once(
        &self,
        repo: &Repo,
        repo_id: &str,
        path: &str,
        is_post: bool,
        service: Option<&str>,
        body: &[u8],
        user_id: &str,
    ) -> Result<GitHttp, Fail> {
        self.sync(repo, repo_id)?;
        match (path, is_post, service) {
            ("info/refs", false, Some("git-upload-pack")) => Ok(GitHttp::Ready {
                content_type: UPLOAD_AD,
                body: advertise_upload(repo)?,
            }),
            ("info/refs", false, Some("git-receive-pack")) => Ok(GitHttp::Ready {
                content_type: RECEIVE_AD,
                body: advertise_receive(local_head(repo)?.as_deref()),
            }),
            ("info/refs", false, _) => {
                Err(Fail::api(StatusCode::BAD_REQUEST, "invalid git service"))
            }
            ("git-upload-pack", true, _) => {
                let body = git_bytes(
                    repo,
                    &["upload-pack", "--stateless-rpc", "."],
                    Some(body),
                    &[],
                )?;
                Ok(GitHttp::Ready {
                    content_type: UPLOAD_RESULT,
                    body,
                })
            }
            ("git-receive-pack", true, _) => self.receive(repo, repo_id, body, user_id),
            _ => Err(Fail::api(StatusCode::NOT_FOUND, "not found")),
        }
    }

    fn receive(
        &self,
        repo: &Repo,
        repo_id: &str,
        body: &[u8],
        user_id: &str,
    ) -> Result<GitHttp, Fail> {
        let (commands, pack) = match split_pack(body) {
            Ok(split) => split,
            Err(err) => {
                eprintln!("memory receive-pack: {err}");
                return Ok(ready(unpack_fail("invalid pack")));
            }
        };
        if commands.is_empty() {
            return Ok(ready(report(Ok(()))));
        }
        if commands.len() != 1 || commands[0].name != "refs/heads/main" {
            return Ok(ready(report(Err("only refs/heads/main can be updated"))));
        }
        let command = &commands[0];
        if command.new == ZERO {
            return Ok(ready(report(Err("refusing to delete main"))));
        }
        if !is_hex(&command.new, 40) {
            return Ok(ready(report(Err("invalid commit"))));
        }
        let head = local_head(repo)?;
        if head.as_deref() == Some(command.new.as_str()) {
            return Ok(ready(report(Ok(()))));
        }
        let old = if command.old == ZERO {
            None
        } else {
            Some(command.old.as_str())
        };
        if head.as_deref() != old {
            return Ok(ready(report(Err("non-fast-forward"))));
        }
        if !pack.is_empty() {
            if let Err(err) = index_pack(repo, pack) {
                eprintln!("memory index-pack: {err}");
                return Ok(ready(unpack_fail("index-pack failed")));
            }
        }
        if let Some(old) = old {
            if !is_ancestor(repo, old, &command.new)? {
                return Ok(ready(report(Err("non-fast-forward"))));
            }
        }
        if let Err(err) = inspect(repo, old, &command.new, user_id) {
            if let Fail::Api(err) = &err {
                if err.status == StatusCode::BAD_REQUEST {
                    return Ok(ready(report(Err(err.message.as_str()))));
                }
            }
            return Err(err);
        }
        match self.publish(repo, repo_id, old, &command.new) {
            Ok(()) => Ok(ready(report(Ok(())))),
            // Conventional phrasing so clients that match `non-fast-forward`
            // (including the Letta memory client) rebase and retry. A lost
            // compare-and-swap is that case: main moved, and this tip did not.
            Err(Fail::Conflict) => Ok(ready(report(Err("non-fast-forward")))),
            Err(err) => Err(err),
        }
    }

    fn publish(
        &self,
        repo: &Repo,
        repo_id: &str,
        old: Option<&str>,
        sha: &str,
    ) -> Result<(), Fail> {
        git_ok(repo, &["update-ref", "refs/cadri/stage", sha], None, &[])?;
        let bundle_path = repo.scratch.join(format!("{sha}.bundle"));
        let _ = fs::remove_file(&bundle_path);
        git_ok(
            repo,
            &[
                "bundle",
                "create",
                &bundle_path.to_string_lossy(),
                "refs/cadri/stage",
            ],
            None,
            &[],
        )?;
        let bytes = fs::read(&bundle_path).map_err(|err| {
            clear_stage(repo);
            Fail::git(err.to_string())
        })?;
        if let Err(err) = self.put_bundle(repo_id, sha, &bytes) {
            clear_stage(repo);
            return Err(err);
        }
        self.wait_for_peer();
        if let Err(err) = self.cas_ref(repo_id, old, sha) {
            clear_stage(repo);
            return Err(err);
        }
        git_ok(repo, &["update-ref", "refs/heads/main", sha], None, &[])?;
        clear_stage(repo);
        if let Ok(mut cache) = self.inner.cache.lock() {
            cache.remove(repo_id);
        }
        Ok(())
    }

    fn sync(&self, repo: &Repo, repo_id: &str) -> Result<(), Fail> {
        let auth = self.read_ref(repo_id)?;
        let local = local_head(repo)?;
        if auth == local {
            return Ok(());
        }
        let Some(sha) = auth else {
            if local.is_some() {
                git_ok(repo, &["update-ref", "-d", "refs/heads/main"], None, &[])?;
            }
            return Ok(());
        };
        if object_exists(repo, &sha)? {
            git_ok(repo, &["update-ref", "refs/heads/main", &sha], None, &[])?;
            return Ok(());
        }
        let bytes = self.load_bundle(repo_id, &sha)?;
        let bundle_path = repo.scratch.join(format!("{sha}.bundle"));
        fs::write(&bundle_path, bytes).map_err(|err| Fail::git(err.to_string()))?;
        git_ok(
            repo,
            &[
                "fetch",
                &bundle_path.to_string_lossy(),
                "+refs/cadri/stage:refs/heads/main",
            ],
            None,
            &[],
        )?;
        if local_head(repo)?.as_deref() != Some(sha.as_str()) {
            return Err(Fail::git(format!("bundle for {sha} did not move main")));
        }
        Ok(())
    }

    fn cached_view(&self, repo: &Repo, repo_id: &str) -> Result<MemoryStore, Fail> {
        let key = local_head(repo)?.unwrap_or_default();
        if let Ok(cache) = self.inner.cache.lock() {
            if let Some((sha, store)) = cache.get(repo_id) {
                if sha == &key {
                    return Ok(store.clone());
                }
            }
        }
        let store = view(repo)?;
        if let Ok(mut cache) = self.inner.cache.lock() {
            cache.insert(repo_id.to_string(), (key, store.clone()));
        }
        Ok(store)
    }

    pub(super) fn read_ref(&self, repo_id: &str) -> Result<Option<String>, Fail> {
        match &self.inner.backend {
            Backend::Directory(store) => read_sha_file(&ref_path(store, repo_id)),
            Backend::S3(books) => books.read_ref(repo_id).map_err(s3_fail),
        }
    }

    pub(super) fn load_bundle(&self, repo_id: &str, sha: &str) -> Result<Vec<u8>, Fail> {
        if let Some(bytes) = self.mount_bundle(repo_id, sha)? {
            return Ok(bytes);
        }
        match &self.inner.backend {
            Backend::Directory(store) => fs::read(obj_path(store, repo_id, sha))
                .map_err(|_| Fail::git(format!("memory bundle {sha} is missing"))),
            Backend::S3(books) => books.get_object(repo_id, sha).map_err(s3_fail),
        }
    }

    pub(super) fn mount_bundle(&self, repo_id: &str, sha: &str) -> Result<Option<Vec<u8>>, Fail> {
        let Some(mount) = &self.inner.mount else {
            return Ok(None);
        };
        if read_sha_file(&mount.join("repos").join(repo_id).join("refs/heads/main"))?.as_deref()
            != Some(sha)
        {
            return Ok(None);
        }
        let path = mount.join("repos").join(repo_id).join("objects").join(sha);
        Ok(fs::read(path).ok())
    }

    pub(super) fn put_bundle(&self, repo_id: &str, sha: &str, bytes: &[u8]) -> Result<(), Fail> {
        match &self.inner.backend {
            Backend::Directory(store) => put_bundle_file(&obj_path(store, repo_id, sha), bytes),
            Backend::S3(books) => books.put_bundle(repo_id, sha, bytes).map_err(s3_fail),
        }
    }

    pub(super) fn cas_ref(
        &self,
        repo_id: &str,
        expected: Option<&str>,
        new_sha: &str,
    ) -> Result<(), Fail> {
        {
            let mut left = self.inner.cas_failures.lock().expect("cas failures");
            if *left > 0 {
                *left -= 1;
                drop(left);
                self.note_conflict();
                return Err(Fail::Conflict);
            }
        }
        let result = match &self.inner.backend {
            Backend::Directory(store) => cas_ref_file(&ref_path(store, repo_id), expected, new_sha),
            Backend::S3(books) => books.cas_ref(repo_id, expected, new_sha).map_err(s3_fail),
        };
        if matches!(result, Err(Fail::Conflict)) {
            self.note_conflict();
        }
        result
    }

    fn wait_for_peer(&self) {
        let gate = self.inner.gate.lock().expect("publish gate").clone();
        if let Some(gate) = gate {
            gate.wait();
        }
    }

    fn note_conflict(&self) {
        let gate = self.inner.gate.lock().expect("publish gate").clone();
        if let Some(gate) = gate {
            gate.note_conflict();
        }
    }
}

/// One-shot meeting point used by the two-server race tests.
pub struct PublishGate {
    inner: Mutex<GateState>,
    cvar: std::sync::Condvar,
    conflicts: std::sync::atomic::AtomicUsize,
}

struct GateState {
    need: u32,
    arrived: u32,
}

impl PublishGate {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(GateState {
                need: 0,
                arrived: 0,
            }),
            cvar: std::sync::Condvar::new(),
            conflicts: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    pub fn arm(&self, parties: u32) {
        let mut state = self.inner.lock().expect("publish gate");
        state.need = parties;
        state.arrived = 0;
    }

    pub fn conflicts(&self) -> usize {
        self.conflicts.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn note_conflict(&self) {
        self.conflicts
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    fn wait(&self) {
        let mut state = self.inner.lock().expect("publish gate");
        if state.need == 0 {
            return;
        }
        state.arrived += 1;
        if state.arrived >= state.need {
            state.need = 0;
            state.arrived = 0;
            self.cvar.notify_all();
            return;
        }
        let start = std::time::Instant::now();
        let limit = Duration::from_secs(15);
        loop {
            let remaining = limit.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                panic!("two publishers did not both reach the compare-and-swap");
            }
            let (guard, result) = self
                .cvar
                .wait_timeout(state, remaining)
                .expect("publish gate");
            state = guard;
            if state.need == 0 {
                return;
            }
            if result.timed_out() {
                panic!("two publishers did not both reach the compare-and-swap");
            }
        }
    }
}
