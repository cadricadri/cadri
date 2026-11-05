use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use axum::http::StatusCode;

use crate::memory::s3::S3Books;
use crate::memory::types::{BlameEntry, Change, MemoryCommit, MemoryStore, MemoryTree};
use crate::ApiError;

use super::commit::{
    apply_changes, build_tree, clean_message, commit_tree, current_sources, Source,
};
use super::error::{into_api, s3_fail, Fail};
use super::git_cli_exec::{
    ensure_repo, git_bytes, git_ok, is_ancestor, local_head, object_exists, Repo,
};
use super::ids::{is_hex, valid_repo_id, ZERO};
use super::protocol::{
    advertise_receive, advertise_upload, index_pack, inspect, ready, report, split_pack,
    unpack_fail, GitHttp, RECEIVE_AD, UPLOAD_AD, UPLOAD_RESULT,
};
use super::snapshot::{snapshot_after, snapshot_from_repo, Snapshot};
use super::store::{cas_ref_file, clear_stage, obj_path, put_bundle_file, read_sha_file, ref_path};
use super::view::{blame_file, read_commits};

type FileBlames = HashMap<String, Vec<BlameEntry>>;

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
    books: Mutex<HashMap<String, Arc<Book>>>,
    logs: Mutex<HashMap<String, (String, Vec<MemoryCommit>)>>,
    blames: Mutex<HashMap<String, (String, FileBlames)>>,
    /// Test-only rendezvous so two servers reach the compare-and-swap together.
    gate: Mutex<Option<Arc<PublishGate>>>,
    /// Test-only. The next this many compare-and-swaps fail, so a UI commit can
    /// be shown exhausting its retries.
    cas_failures: Mutex<u32>,
}

/// One book in this process. Writers hold `writer`; readers only clone the snapshot
/// `Arc`, so a slow write never blocks a read.
struct Book {
    writer: Mutex<WriterState>,
    snapshot: RwLock<Option<Arc<Snapshot>>>,
}

struct WriterState;

impl Book {
    fn current(&self) -> Option<Arc<Snapshot>> {
        self.snapshot
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }
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
                books: Mutex::new(HashMap::new()),
                logs: Mutex::new(HashMap::new()),
                blames: Mutex::new(HashMap::new()),
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

    /// Tree for the memories page, from memory. Never takes the writer lock once
    /// the book is loaded.
    pub(crate) fn read_store(&self, repo_id: &str) -> Result<MemoryStore, ApiError> {
        self.snapshot(repo_id)
            .map(|snapshot| snapshot.to_store())
            .map_err(into_api)
    }

    pub(crate) fn read_tree(
        &self,
        repo_id: &str,
        open: Option<&str>,
    ) -> Result<MemoryTree, ApiError> {
        self.snapshot(repo_id)
            .map(|snapshot| snapshot.to_tree(open))
            .map_err(into_api)
    }

    /// One file's text by blob SHA. Only blobs in this book's tip are served.
    pub(crate) fn read_blob(&self, repo_id: &str, sha: &str) -> Result<Arc<str>, ApiError> {
        if sha.len() != 40 || !sha.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid blob id"));
        }
        let snapshot = self.snapshot(repo_id).map_err(into_api)?;
        snapshot
            .blobs
            .get(sha)
            .cloned()
            .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "not found"))
    }

    pub(crate) fn read_tip(&self, repo_id: &str) -> Result<(Option<String>, u64), ApiError> {
        let snapshot = self.snapshot(repo_id).map_err(into_api)?;
        Ok((snapshot.tip.clone(), snapshot.seq))
    }

    pub(crate) fn read_log(&self, repo_id: &str) -> Result<Vec<MemoryCommit>, ApiError> {
        self.read_history(repo_id).map_err(into_api)
    }

    pub(crate) fn read_blame(
        &self,
        repo_id: &str,
        path: &str,
    ) -> Result<Vec<BlameEntry>, ApiError> {
        self.read_file_blame(repo_id, path).map_err(into_api)
    }

    /// Test helper: hold the write lock. Readers must not wait on this.
    #[cfg(test)]
    pub(crate) fn hold_writer(
        &self,
        repo_id: &str,
        how_long: Duration,
        entered: &std::sync::atomic::AtomicBool,
    ) {
        self.with_writer(repo_id, |_repo, _book, _writer| {
            entered.store(true, std::sync::atomic::Ordering::SeqCst);
            std::thread::sleep(how_long);
            Ok(())
        })
        .unwrap();
    }

    pub(crate) fn commit(
        &self,
        repo_id: &str,
        user_id: &str,
        label: &str,
        message: &str,
        changes: &[Change],
    ) -> Result<MemoryStore, ApiError> {
        self.commit_book(repo_id, user_id, label, message, changes)
            .map(|snapshot| snapshot.to_store())
    }

    /// Same as `commit`, answered as the new tree.
    pub(crate) fn commit_tree_view(
        &self,
        repo_id: &str,
        user_id: &str,
        label: &str,
        message: &str,
        changes: &[Change],
    ) -> Result<MemoryTree, ApiError> {
        self.commit_book(repo_id, user_id, label, message, changes)
            .map(|snapshot| MemoryTree {
                open: None,
                ..snapshot.to_tree(None)
            })
    }

    fn commit_book(
        &self,
        repo_id: &str,
        user_id: &str,
        label: &str,
        message: &str,
        changes: &[Change],
    ) -> Result<Arc<Snapshot>, ApiError> {
        for _ in 0..8 {
            match self.with_writer(repo_id, |repo, book, _writer| {
                match self.commit_once(repo, repo_id, book, user_id, label, message, changes) {
                    Err(Fail::Conflict) => {
                        self.refresh(repo, repo_id, book)?;
                        Err(Fail::Conflict)
                    }
                    Err(err) => Err(err),
                    // The writer lock is still held, so this is the snapshot just installed.
                    Ok(_) => loaded(book),
                }
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
        self.with_writer(repo_id, |repo, book, _writer| {
            self.git_once(repo, repo_id, book, path, is_post, service, body, user_id)
        })
        .map_err(into_api)
    }

    fn book(&self, repo_id: &str) -> Result<Arc<Book>, Fail> {
        if !valid_repo_id(repo_id) {
            return Err(Fail::api(
                StatusCode::INTERNAL_SERVER_ERROR,
                "invalid memory repo",
            ));
        }
        let mut books = self
            .inner
            .books
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        Ok(books
            .entry(repo_id.to_string())
            .or_insert_with(|| {
                Arc::new(Book {
                    writer: Mutex::new(WriterState),
                    snapshot: RwLock::new(None),
                })
            })
            .clone())
    }

    fn repo(&self, repo_id: &str) -> Result<Repo, Fail> {
        let scratch = self.inner.scratch.join(repo_id);
        fs::create_dir_all(&scratch).map_err(|err| Fail::git(err.to_string()))?;
        Ok(Repo {
            git_dir: self.inner.repos.join(format!("{repo_id}.git")),
            scratch,
            jail: self.inner.jail,
        })
    }

    /// The book's current tip. Loads it from the durable store on first use.
    fn snapshot(&self, repo_id: &str) -> Result<Arc<Snapshot>, Fail> {
        let book = self.book(repo_id)?;
        if let Some(snapshot) = book.current() {
            return Ok(snapshot);
        }
        self.with_writer(repo_id, |_repo, book, _writer| {
            book.current()
                .ok_or_else(|| Fail::git("memory snapshot was not loaded"))
        })
    }

    /// Runs `f` as the only writer of the book, with its snapshot loaded.
    fn with_writer<T>(
        &self,
        repo_id: &str,
        f: impl FnOnce(&Repo, &Book, &mut WriterState) -> Result<T, Fail>,
    ) -> Result<T, Fail> {
        let book = self.book(repo_id)?;
        let mut writer = book.writer.lock().unwrap_or_else(|err| err.into_inner());
        let repo = self.repo(repo_id)?;
        ensure_repo(&repo)?;
        if book.current().is_none() {
            self.refresh(&repo, repo_id, &book)?;
        }
        f(&repo, &book, &mut writer)
    }

    /// Catches the local repo up with the durable store and rebuilds the snapshot.
    fn refresh(&self, repo: &Repo, repo_id: &str, book: &Book) -> Result<(), Fail> {
        self.sync(repo, repo_id)?;
        let tip = local_head(repo)?;
        let previous = book.current();
        let seq = previous.as_ref().map_or(0, |snapshot| snapshot.seq);
        let snapshot = snapshot_from_repo(repo, tip.as_deref(), seq, previous.as_deref())?;
        self.install(repo_id, book, snapshot);
        Ok(())
    }

    fn install(&self, repo_id: &str, book: &Book, snapshot: Snapshot) {
        *book.snapshot.write().unwrap_or_else(|err| err.into_inner()) = Some(Arc::new(snapshot));
        if let Ok(mut logs) = self.inner.logs.lock() {
            logs.remove(repo_id);
        }
        if let Ok(mut blames) = self.inner.blames.lock() {
            blames.remove(repo_id);
        }
    }

    /// The tip was published, so the snapshot must follow. If it cannot be built the
    /// book is marked unloaded and reloads from the durable store next time.
    fn install_published(
        &self,
        repo: &Repo,
        repo_id: &str,
        book: &Book,
        previous: &Snapshot,
        tip: &str,
    ) -> Result<Arc<Snapshot>, Fail> {
        match snapshot_from_repo(repo, Some(tip), previous.seq, Some(previous)) {
            Ok(snapshot) => {
                self.install(repo_id, book, snapshot);
                book.current()
                    .ok_or_else(|| Fail::git("memory snapshot was not installed"))
            }
            Err(err) => {
                *book.snapshot.write().unwrap_or_else(|err| err.into_inner()) = None;
                Err(err)
            }
        }
    }

    #[allow(clippy::too_many_arguments)] // one commit: book, author, message, and changes
    fn commit_once(
        &self,
        repo: &Repo,
        repo_id: &str,
        book: &Book,
        user_id: &str,
        label: &str,
        message: &str,
        changes: &[Change],
    ) -> Result<MemoryStore, Fail> {
        let snapshot = loaded(book)?;
        let message = clean_message(message)?;
        let before = current_sources(&snapshot);
        let mut files = before.clone();
        apply_changes(&mut files, changes)?;
        // Writing what a file already holds is not a change.
        for (path, source) in files.iter_mut() {
            if let (Source::Text(text), Some(Source::Blob(blob))) = (&*source, before.get(path)) {
                if snapshot.blobs.get(blob).is_some_and(|old| **old == **text) {
                    *source = Source::Blob(blob.clone());
                }
            }
        }
        if files == before {
            return Ok(snapshot.to_store());
        }
        let parent = snapshot.tip.as_deref();
        let built = build_tree(repo, parent, &before, &files)?;
        let sha = commit_tree(repo, &built.tree, parent, &message, user_id, label)?;
        self.publish(repo, repo_id, parent, &sha)?;
        let next = match snapshot_after(&snapshot, &sha, &built.files, built.new_blobs) {
            Ok(next) => next,
            Err(err) => {
                *book.snapshot.write().unwrap_or_else(|err| err.into_inner()) = None;
                return Err(err);
            }
        };
        self.install(repo_id, book, next);
        let updated = book
            .current()
            .ok_or_else(|| Fail::git("memory snapshot was not installed"))?;
        Ok(updated.to_store())
    }

    #[allow(clippy::too_many_arguments)] // path, verb, service, and body are one git HTTP call
    fn git_once(
        &self,
        repo: &Repo,
        repo_id: &str,
        book: &Book,
        path: &str,
        is_post: bool,
        service: Option<&str>,
        body: &[u8],
        user_id: &str,
    ) -> Result<GitHttp, Fail> {
        let snapshot = loaded(book)?;
        match (path, is_post, service) {
            ("info/refs", false, Some("git-upload-pack")) => Ok(GitHttp::Ready {
                content_type: UPLOAD_AD,
                body: advertise_upload(repo)?,
            }),
            ("info/refs", false, Some("git-receive-pack")) => Ok(GitHttp::Ready {
                content_type: RECEIVE_AD,
                body: advertise_receive(snapshot.tip.as_deref()),
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
            ("git-receive-pack", true, _) => {
                self.receive(repo, repo_id, book, &snapshot, body, user_id)
            }
            _ => Err(Fail::api(StatusCode::NOT_FOUND, "not found")),
        }
    }

    fn receive(
        &self,
        repo: &Repo,
        repo_id: &str,
        book: &Book,
        snapshot: &Snapshot,
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
        let head = snapshot.tip.clone();
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
        if let Err(err) = inspect(repo, snapshot, old, &command.new, user_id) {
            if let Fail::Api(err) = &err {
                if err.status == StatusCode::BAD_REQUEST {
                    return Ok(ready(report(Err(err.message.as_str()))));
                }
            }
            return Err(err);
        }
        match self.publish(repo, repo_id, old, &command.new) {
            Ok(()) => {
                self.install_published(repo, repo_id, book, snapshot, &command.new)?;
                Ok(ready(report(Ok(()))))
            }
            // Conventional phrasing so clients that match `non-fast-forward`
            // (including the Letta memory client) rebase and retry. A lost
            // compare-and-swap is that case: main moved, and this tip did not.
            Err(Fail::Conflict) => {
                self.refresh(repo, repo_id, book)?;
                // A write whose response was lost may already have landed.
                if book.current().and_then(|now| now.tip.clone()).as_deref()
                    == Some(command.new.as_str())
                {
                    return Ok(ready(report(Ok(()))));
                }
                Ok(ready(report(Err("non-fast-forward"))))
            }
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

    fn read_history(&self, repo_id: &str) -> Result<Vec<MemoryCommit>, Fail> {
        let snapshot = self.snapshot(repo_id)?;
        let Some(key) = snapshot.tip.clone() else {
            return Ok(Vec::new());
        };
        if let Ok(cache) = self.inner.logs.lock() {
            if let Some((sha, commits)) = cache.get(repo_id) {
                if sha == &key {
                    return Ok(commits.clone());
                }
            }
        }
        let commits = read_commits(&self.repo(repo_id)?)?;
        if let Ok(mut cache) = self.inner.logs.lock() {
            cache.insert(repo_id.to_string(), (key, commits.clone()));
        }
        Ok(commits)
    }

    fn read_file_blame(&self, repo_id: &str, path: &str) -> Result<Vec<BlameEntry>, Fail> {
        if !crate::memory::types::memory_path_ok(path) {
            return Err(Fail::api(StatusCode::BAD_REQUEST, "invalid memory path"));
        }
        let snapshot = self.snapshot(repo_id)?;
        let Some(key) = snapshot.tip.clone() else {
            return Err(Fail::api(StatusCode::NOT_FOUND, "memory file not found"));
        };
        if let Ok(cache) = self.inner.blames.lock() {
            if let Some((sha, files)) = cache.get(repo_id) {
                if sha == &key {
                    if let Some(blame) = files.get(path) {
                        return Ok(blame.clone());
                    }
                }
            }
        }
        let blame = blame_file(&self.repo(repo_id)?, path)?;
        if let Ok(mut cache) = self.inner.blames.lock() {
            let entry = cache
                .entry(repo_id.to_string())
                .or_insert_with(|| (key.clone(), HashMap::new()));
            if entry.0 != key {
                *entry = (key, HashMap::new());
            }
            entry.1.insert(path.to_string(), blame.clone());
        }
        Ok(blame)
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

fn loaded(book: &Book) -> Result<Arc<Snapshot>, Fail> {
    book.current()
        .ok_or_else(|| Fail::git("memory snapshot was not loaded"))
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
