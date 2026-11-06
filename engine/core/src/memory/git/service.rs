use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, RwLock};
use std::time::Duration;

use axum::http::StatusCode;

use crate::memory::s3::S3Books;
use crate::memory::types::{
    BlameEntry, Change, CommitDetail, CommittedTree, EditRequest, MemoryCommit, MemoryStore,
    MemoryTree,
};
use crate::ApiError;

use super::author::valid_session;
use super::bundle::parse_bundle_header;
use super::commit::{
    apply_changes, check_content, clean_message, commit_tree, current_sources, valid_agent,
    CommitAuthor, Source, TreeBuilder,
};
use super::durable::{checkpoint_key, log_key, seq_of, Created, DirDurable, Durable};
use super::error::{into_api, s3_fail, Fail};
use super::git_cli_exec::{ensure_repo, git_bytes, git_ok, is_ancestor, local_head, Repo};
use super::ids::{is_hex, valid_repo_id, ZERO};
use super::protocol::{
    advertise_receive, advertise_upload, index_pack, inspect, ready, report, split_pack,
    unpack_fail, upload_args, GitHttp, RECEIVE_AD, UPLOAD_AD, UPLOAD_RESULT,
};
use super::snapshot::{snapshot_after, snapshot_from_repo, Snapshot};
use super::store::{clear_stage, obj_path, read_sha_file, ref_path};
use super::view::{blame_file, read_commit_detail, read_commits};

type FileBlames = HashMap<String, Vec<BlameEntry>>;

pub(super) enum Backend {
    Directory(PathBuf),
    S3(Arc<S3Books>),
}

/// Log entries downloaded at once while loading a book.
const LOG_FETCHES: usize = 8;

/// A checkpoint is written once this many log entries, or this many bytes of them,
/// have piled up since the last one.
const CHECKPOINT_ENTRIES: u64 = 100;
const CHECKPOINT_BYTES: u64 = 8 * 1024 * 1024;

/// How long an edit waits for the writer that took it from the queue to answer.
const EDIT_TIMEOUT: Duration = Duration::from_secs(30);

struct Inner {
    repos: PathBuf,
    /// Legacy layout, read only to migrate a book.
    backend: Backend,
    durable: Box<dyn Durable>,
    scratch: PathBuf,
    jail: bool,
    books: Mutex<HashMap<String, Arc<Book>>>,
    logs: Mutex<HashMap<String, (String, Vec<MemoryCommit>)>>,
    blames: Mutex<HashMap<String, (String, FileBlames)>>,
    /// Test-only rendezvous so two servers reach the log create together.
    gate: Mutex<Option<Arc<PublishGate>>>,
    /// Test-only. The next this many log creates find their key taken, so a UI
    /// commit can be shown exhausting its retries.
    log_failures: Mutex<u32>,
    /// Test-only. The next this many checkpoint uploads fail.
    checkpoint_failures: Mutex<u32>,
}

/// One book in this process. Writers hold `writer`; readers only clone the snapshot
/// `Arc`, so a slow write never blocks a read.
struct Book {
    writer: Mutex<WriterState>,
    snapshot: RwLock<Option<Arc<Snapshot>>>,
    /// Edits waiting for the writer. Whoever holds `writer` next commits them all.
    queue: Mutex<Vec<Pending>>,
    /// A checkpoint upload is running, so no second one starts.
    checkpointing: AtomicBool,
}

struct Pending {
    request: EditRequest,
    reply: mpsc::SyncSender<Result<EditOutcome, ApiError>>,
}

/// One edit's answer: its own commit (`None` for a no-op) and the book after the
/// batch it was committed in.
pub(crate) struct EditOutcome {
    pub commit: Option<String>,
    snapshot: Arc<Snapshot>,
}

impl EditOutcome {
    pub(crate) fn store(&self) -> MemoryStore {
        self.snapshot.to_store()
    }

    pub(crate) fn tree(&self) -> CommittedTree {
        CommittedTree {
            tree: MemoryTree {
                open: None,
                ..self.snapshot.to_tree(None)
            },
            commit: self.commit.clone(),
        }
    }
}

#[derive(Default)]
struct WriterState {
    /// Last log entry applied to the local repo and the snapshot.
    seq: u64,
    /// Highest checkpoint known to be durable.
    checkpoint_seq: u64,
    log_bytes_since_checkpoint: u64,
}

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
    pub fn open_local(root: impl Into<PathBuf>, jail: bool) -> Self {
        let root = root.into();
        Self::new(
            root.join("repos"),
            Backend::Directory(root.join("store")),
            root.join("scratch"),
            jail,
        )
    }

    pub fn open_s3(root: impl Into<PathBuf>, jail: bool, books: S3Books) -> Self {
        let root = root.into();
        Self::new(
            root.join("repos"),
            Backend::S3(Arc::new(books)),
            root.join("scratch"),
            jail,
        )
    }

    pub(super) fn new(repos: PathBuf, backend: Backend, scratch: PathBuf, jail: bool) -> Self {
        let durable: Box<dyn Durable> = match &backend {
            Backend::Directory(store) => Box::new(DirDurable::new(store.clone())),
            Backend::S3(books) => Box::new(Arc::clone(books)),
        };
        MemoryService {
            inner: Arc::new(Inner {
                repos,
                backend,
                durable,
                scratch,
                jail,
                books: Mutex::new(HashMap::new()),
                logs: Mutex::new(HashMap::new()),
                blames: Mutex::new(HashMap::new()),
                gate: Mutex::new(None),
                log_failures: Mutex::new(0),
                checkpoint_failures: Mutex::new(0),
            }),
        }
    }

    /// Test helper: the next `times` log creates find their key taken by another
    /// writer, without anything being stored.
    pub fn fail_next_log_create(&self, times: u32) {
        *self.inner.log_failures.lock().expect("log failures") = times;
    }

    /// Test helper: the next `times` checkpoint uploads fail.
    pub fn fail_next_checkpoint_upload(&self, times: u32) {
        *self
            .inner
            .checkpoint_failures
            .lock()
            .expect("checkpoint failures") = times;
    }

    /// Test helper: directory store with explicit git/scratch paths.
    pub fn open_at(repos: PathBuf, store: PathBuf, scratch: PathBuf) -> Self {
        Self::new(repos, Backend::Directory(store), scratch, false)
    }

    /// Test helper: two servers wait in `publish` until `parties` arrive, then
    /// exactly one log create succeeds.
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

    pub(crate) fn read_commit(&self, repo_id: &str, sha: &str) -> Result<CommitDetail, ApiError> {
        let snapshot = self.snapshot(repo_id).map_err(into_api)?;
        if snapshot.tip.is_none() {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "memory commit not found",
            ));
        }
        let repo = self.repo(repo_id).map_err(into_api)?;
        read_commit_detail(&repo, sha).map_err(into_api)
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

    /// Test helper: `(seq, checkpoint_seq, log_bytes_since_checkpoint)`.
    #[cfg(test)]
    pub(crate) fn writer_state(&self, repo_id: &str) -> (u64, u64, u64) {
        self.with_writer(repo_id, |_repo, _book, writer| {
            Ok((
                writer.seq,
                writer.checkpoint_seq,
                writer.log_bytes_since_checkpoint,
            ))
        })
        .unwrap()
    }

    /// Test helper: a human edit without a base, answered as the legacy store.
    #[cfg(test)]
    pub(crate) fn commit(
        &self,
        repo_id: &str,
        user_id: &str,
        label: &str,
        message: &str,
        changes: &[Change],
    ) -> Result<MemoryStore, ApiError> {
        self.edit(
            repo_id,
            EditRequest {
                user_id: user_id.to_string(),
                label: label.to_string(),
                message: message.to_string(),
                changes: changes.to_vec(),
                base: None,
                agent: None,
                session_id: None,
            },
        )
        .map(|outcome| outcome.store())
    }

    /// Applies `request` on top of whatever the tip is when its turn comes.
    /// Concurrent edits are committed together, one commit each, in one log entry.
    pub(crate) fn edit(
        &self,
        repo_id: &str,
        request: EditRequest,
    ) -> Result<EditOutcome, ApiError> {
        if request
            .agent
            .as_deref()
            .is_some_and(|agent| !valid_agent(agent))
        {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid agent"));
        }
        if request
            .session_id
            .as_deref()
            .is_some_and(|session| !valid_session(session))
        {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid session id"));
        }
        self.submit(repo_id, request)
    }

    fn submit(&self, repo_id: &str, request: EditRequest) -> Result<EditOutcome, ApiError> {
        let book = self.book(repo_id).map_err(into_api)?;
        let (tx, rx) = mpsc::sync_channel(1);
        book.queue
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .push(Pending { request, reply: tx });
        {
            let mut state = book.writer.lock().unwrap_or_else(|err| err.into_inner());
            // A previous holder may have committed this request already.
            if let Ok(result) = rx.try_recv() {
                return result;
            }
            let batch =
                std::mem::take(&mut *book.queue.lock().unwrap_or_else(|err| err.into_inner()));
            self.commit_batch(repo_id, &book, &mut state, batch);
        }
        rx.recv_timeout(EDIT_TIMEOUT).unwrap_or_else(|_| {
            Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "memory write timed out",
            ))
        })
    }

    /// Sends exactly one reply per pending edit.
    fn commit_batch(
        &self,
        repo_id: &str,
        book: &Book,
        state: &mut WriterState,
        batch: Vec<Pending>,
    ) {
        if batch.is_empty() {
            return;
        }
        let (requests, replies): (Vec<EditRequest>, Vec<_>) = batch
            .into_iter()
            .map(|pending| (pending.request, pending.reply))
            .unzip();
        let results = self.run_batch(repo_id, book, state, &requests);
        for (reply, result) in replies.into_iter().zip(results) {
            let _ = reply.send(result);
        }
    }

    /// One commit per edit on top of the running tip, published as one log entry.
    /// A lost log create catches up and rebuilds the whole batch once.
    fn run_batch(
        &self,
        repo_id: &str,
        book: &Book,
        state: &mut WriterState,
        requests: &[EditRequest],
    ) -> Vec<Result<EditOutcome, ApiError>> {
        let all = |err: Fail| {
            let err = into_api(err);
            requests.iter().map(|_| Err(err.clone())).collect()
        };
        let repo = match self.repo(repo_id) {
            Ok(repo) => repo,
            Err(err) => return all(err),
        };
        if book.current().is_none() {
            if let Err(err) = self.hydrate(&repo, repo_id, book, state) {
                return all(err);
            }
        }
        let mut made = Vec::new();
        for _ in 0..2 {
            let start = match loaded(book) {
                Ok(start) => start,
                Err(err) => return all(err),
            };
            let next;
            (next, made) = build_batch(&repo, &start, requests);
            let Some(next) = next else {
                return answer(made, &start);
            };
            let tip = next.tip.clone().unwrap_or_default();
            match self.publish(&repo, repo_id, book, state, start.tip.as_deref(), &tip) {
                Ok(()) => {
                    self.install(
                        repo_id,
                        book,
                        Snapshot {
                            seq: state.seq,
                            ..next
                        },
                    );
                    return match loaded(book) {
                        Ok(installed) => answer(made, &installed),
                        Err(err) => all(err),
                    };
                }
                // `publish` caught the book up; rebuild every edit on the new tip.
                Err(Fail::Conflict) => continue,
                Err(Fail::Api(err)) => return fail_committed(made, err),
            }
        }
        fail_committed(
            made,
            ApiError::new(StatusCode::CONFLICT, "memory book changed"),
        )
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
        match (path, is_post, service) {
            ("info/refs", false, Some("git-upload-pack")) | ("git-upload-pack", true, _) => {
                self.serve_upload(repo_id, path, body).map_err(into_api)
            }
            ("info/refs", false, Some("git-receive-pack")) => self
                .snapshot(repo_id)
                .map(|snapshot| GitHttp::Ready {
                    content_type: RECEIVE_AD,
                    body: advertise_receive(snapshot.tip.as_deref()),
                })
                .map_err(into_api),
            _ => self
                .with_writer(repo_id, |repo, book, writer| {
                    self.git_once(
                        repo, repo_id, book, writer, path, is_post, service, body, user_id,
                    )
                })
                .map_err(into_api),
        }
    }

    /// Clone and fetch: waits for the book to load, never for a writer. Git writes
    /// objects before it moves a ref, so it sees the old tip or the new one.
    fn serve_upload(&self, repo_id: &str, path: &str, body: &[u8]) -> Result<GitHttp, Fail> {
        self.snapshot(repo_id)?;
        let repo = self.repo(repo_id)?;
        if path == "info/refs" {
            return Ok(GitHttp::Ready {
                content_type: UPLOAD_AD,
                body: advertise_upload(&repo)?,
            });
        }
        let body = git_bytes(
            &repo,
            &upload_args(&["--stateless-rpc", "."]),
            Some(body),
            &[],
        )?;
        Ok(GitHttp::Ready {
            content_type: UPLOAD_RESULT,
            body,
        })
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
                    writer: Mutex::new(WriterState::default()),
                    snapshot: RwLock::new(None),
                    queue: Mutex::new(Vec::new()),
                    checkpointing: AtomicBool::new(false),
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
        if book.current().is_none() {
            self.hydrate(&repo, repo_id, &book, &mut writer)?;
        }
        f(&repo, &book, &mut writer)
    }

    /// Loads the book into a fresh local repo: the latest checkpoint, then every log
    /// entry after it. A legacy book (tip ref plus full bundle) becomes checkpoint 0.
    fn hydrate(
        &self,
        repo: &Repo,
        repo_id: &str,
        book: &Book,
        state: &mut WriterState,
    ) -> Result<(), Fail> {
        if repo.git_dir.exists() {
            fs::remove_dir_all(&repo.git_dir).map_err(|err| Fail::git(err.to_string()))?;
        }
        if repo.scratch.exists() {
            fs::remove_dir_all(&repo.scratch).map_err(|err| Fail::git(err.to_string()))?;
        }
        fs::create_dir_all(&repo.scratch).map_err(|err| Fail::git(err.to_string()))?;
        ensure_repo(repo)?;
        *state = WriterState::default();

        let durable = &*self.inner.durable;
        let mut checkpoint = durable
            .list(repo_id, "checkpoint/", None)?
            .iter()
            .filter_map(|key| seq_of(key))
            .max();
        let after = checkpoint.map(log_key);
        let entries = durable.list(repo_id, "log/", after.as_deref())?;
        let mut tip = None;
        let checkpoint_bytes = match checkpoint {
            Some(seq) => Some(durable.get(repo_id, &checkpoint_key(seq))?.ok_or_else(|| {
                broken(format!("memory checkpoint {seq} disappeared while loading"))
            })?),
            None if entries.is_empty() => {
                let migrated = self.migrate(repo_id)?;
                if migrated.is_some() {
                    checkpoint = Some(0);
                }
                migrated
            }
            None => None,
        };
        if let (Some(seq), Some(bytes)) = (checkpoint, checkpoint_bytes) {
            let name = format!("checkpoint-{seq:020}");
            tip = Some(apply_bundle(repo, &bytes, &name, None)?);
        }
        let base = checkpoint.unwrap_or(0);
        let (seq, tip, bytes) = self.apply_log(repo, repo_id, &entries, base, tip)?;
        let snapshot = snapshot_from_repo(repo, tip.as_deref(), seq, None)?;
        *state = WriterState {
            seq,
            checkpoint_seq: base,
            log_bytes_since_checkpoint: bytes,
        };
        self.install(repo_id, book, snapshot);
        Ok(())
    }

    /// The legacy book's full bundle, stored as checkpoint 0. `None` when the book
    /// has no legacy tip either, which is a new, empty book.
    fn migrate(&self, repo_id: &str) -> Result<Option<Vec<u8>>, Fail> {
        let Some(tip) = self.read_ref(repo_id)? else {
            return Ok(None);
        };
        let legacy = self.load_bundle(repo_id, &tip)?;
        let key = checkpoint_key(0);
        let bytes = match self.inner.durable.create(repo_id, &key, &legacy)? {
            Created::Stored => legacy,
            // Another process migrated first; its checkpoint is the one later log
            // entries build on.
            Created::Exists => self
                .inner
                .durable
                .get(repo_id, &key)?
                .ok_or_else(|| broken("memory checkpoint 0 disappeared while migrating"))?,
        };
        let header = parse_bundle_header(&bytes)?;
        if !matches!(header.refs.as_slice(), [(sha, _)] if *sha == tip) {
            return Err(broken(format!(
                "memory checkpoint 0 does not hold the legacy tip {tip}"
            )));
        }
        Ok(Some(bytes))
    }

    /// Applies log entries `keys` (seq `base + 1`, `base + 2`, ...) in order on top of
    /// `tip`. Returns the last seq, the new tip, and the bytes applied.
    fn apply_log(
        &self,
        repo: &Repo,
        repo_id: &str,
        keys: &[String],
        base: u64,
        mut tip: Option<String>,
    ) -> Result<(u64, Option<String>, u64), Fail> {
        let keys: Vec<&String> = keys.iter().filter(|key| seq_of(key).is_some()).collect();
        for (index, key) in keys.iter().enumerate() {
            let want = base + 1 + index as u64;
            if seq_of(key) != Some(want) {
                return Err(broken(format!("memory log gap after {}", want - 1)));
            }
        }
        if keys.is_empty() {
            return Ok((base, tip, 0));
        }
        let durable = &*self.inner.durable;
        let next = AtomicUsize::new(0);
        let stop = AtomicBool::new(false);
        let (tx, rx) = mpsc::channel::<(usize, Result<Vec<u8>, Fail>)>();
        let mut seq = base;
        let mut total = 0u64;
        std::thread::scope(|scope| {
            for _ in 0..keys.len().min(LOG_FETCHES) {
                let tx = tx.clone();
                let (keys, next, stop) = (&keys, &next, &stop);
                scope.spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(key) = keys.get(index) else {
                            break;
                        };
                        let got = durable.get(repo_id, key).and_then(|bytes| {
                            bytes.ok_or_else(|| {
                                broken(format!("memory {key} disappeared while loading"))
                            })
                        });
                        if tx.send((index, got)).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(tx);
            let mut arrived = BTreeMap::new();
            let result = (|| {
                while (seq - base) < keys.len() as u64 {
                    let (index, got) = rx
                        .recv()
                        .map_err(|_| Fail::git("memory log download stopped"))?;
                    arrived.insert(index, got);
                    while let Some(got) = arrived.remove(&((seq - base) as usize)) {
                        let bytes = got?;
                        let name = format!("log-{:020}", seq + 1);
                        tip = Some(apply_bundle(repo, &bytes, &name, tip.as_deref())?);
                        seq += 1;
                        total += bytes.len() as u64;
                    }
                }
                Ok(())
            })();
            if result.is_err() {
                stop.store(true, Ordering::Relaxed);
            }
            result
        })?;
        Ok((seq, tip, total))
    }

    /// Another writer appended to the log. Applies its entries and installs the
    /// resulting snapshot. On failure the book is unloaded so the next use reloads it.
    fn catch_up(
        &self,
        repo: &Repo,
        repo_id: &str,
        book: &Book,
        state: &mut WriterState,
    ) -> Result<(), Fail> {
        let result = (|| {
            let previous = loaded(book)?;
            let keys = self
                .inner
                .durable
                .list(repo_id, "log/", Some(&log_key(state.seq)))?;
            let (seq, tip, bytes) =
                self.apply_log(repo, repo_id, &keys, state.seq, previous.tip.clone())?;
            if seq == state.seq {
                return Ok(());
            }
            let snapshot = snapshot_from_repo(repo, tip.as_deref(), seq, Some(&previous))?;
            state.seq = seq;
            state.log_bytes_since_checkpoint += bytes;
            self.install(repo_id, book, snapshot);
            Ok(())
        })();
        if result.is_err() {
            self.unload(book);
        }
        result
    }

    fn unload(&self, book: &Book) {
        *book.snapshot.write().unwrap_or_else(|err| err.into_inner()) = None;
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
        state: &WriterState,
        previous: &Snapshot,
        tip: &str,
    ) -> Result<Arc<Snapshot>, Fail> {
        match snapshot_from_repo(repo, Some(tip), state.seq, Some(previous)) {
            Ok(snapshot) => {
                self.install(repo_id, book, snapshot);
                book.current()
                    .ok_or_else(|| Fail::git("memory snapshot was not installed"))
            }
            Err(err) => {
                self.unload(book);
                Err(err)
            }
        }
    }

    #[allow(clippy::too_many_arguments)] // path, verb, service, and body are one git HTTP call
    fn git_once(
        &self,
        repo: &Repo,
        repo_id: &str,
        book: &Book,
        state: &mut WriterState,
        path: &str,
        is_post: bool,
        service: Option<&str>,
        body: &[u8],
        user_id: &str,
    ) -> Result<GitHttp, Fail> {
        let snapshot = loaded(book)?;
        match (path, is_post, service) {
            ("info/refs", false, _) => {
                Err(Fail::api(StatusCode::BAD_REQUEST, "invalid git service"))
            }
            ("git-receive-pack", true, _) => {
                self.receive(repo, repo_id, book, state, &snapshot, body, user_id)
            }
            _ => Err(Fail::api(StatusCode::NOT_FOUND, "not found")),
        }
    }

    #[allow(clippy::too_many_arguments)] // one push: book, writer, and the request
    fn receive(
        &self,
        repo: &Repo,
        repo_id: &str,
        book: &Book,
        state: &mut WriterState,
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
        match self.publish(repo, repo_id, book, state, old, &command.new) {
            Ok(()) => {
                self.install_published(repo, repo_id, book, state, snapshot, &command.new)?;
                Ok(ready(report(Ok(()))))
            }
            // Conventional phrasing so clients that match `non-fast-forward`
            // rebase and retry. A lost log create is that case: main moved,
            // and this tip did not.
            Err(Fail::Conflict) => {
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

    /// Makes `sha` (a descendant of `old`) durable as log entry `seq + 1`, then moves
    /// `main`. If another writer took that entry, catches up and returns
    /// `Fail::Conflict`.
    fn publish(
        &self,
        repo: &Repo,
        repo_id: &str,
        book: &Book,
        state: &mut WriterState,
        old: Option<&str>,
        sha: &str,
    ) -> Result<(), Fail> {
        let bytes = match log_bundle(repo, old, sha) {
            Ok(bytes) => bytes,
            Err(err) => {
                clear_stage(repo);
                return Err(err);
            }
        };
        let key = log_key(state.seq + 1);
        self.wait_for_peer();
        let ours = if self.take_log_failure() {
            false
        } else {
            match self.inner.durable.create(repo_id, &key, &bytes) {
                Ok(Created::Stored) => true,
                // A retry whose earlier attempt was stored before its response was lost.
                Ok(Created::Exists) => self.log_entry_holds(repo_id, &key, sha)?,
                Err(err) => {
                    clear_stage(repo);
                    return Err(err);
                }
            }
        };
        clear_stage(repo);
        if !ours {
            self.note_conflict();
            self.catch_up(repo, repo_id, book, state)?;
            return Err(Fail::Conflict);
        }
        state.seq += 1;
        state.log_bytes_since_checkpoint += bytes.len() as u64;
        if let Err(err) = git_ok(repo, &["update-ref", "refs/heads/main", sha], None, &[]) {
            self.unload(book);
            return Err(err);
        }
        self.maybe_checkpoint(repo, repo_id, book, state, sha);
        Ok(())
    }

    /// Starts a background checkpoint of `tip` when enough log has piled up. The ref
    /// pins the commit's objects while the bundle is written without the writer lock.
    fn maybe_checkpoint(
        &self,
        repo: &Repo,
        repo_id: &str,
        book: &Book,
        state: &WriterState,
        tip: &str,
    ) {
        let due = state.seq - state.checkpoint_seq >= CHECKPOINT_ENTRIES
            || state.log_bytes_since_checkpoint >= CHECKPOINT_BYTES;
        if !due
            || book
                .checkpointing
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
        {
            return;
        }
        let seq = state.seq;
        if let Err(err) = git_ok(
            repo,
            &["update-ref", "refs/cadri/checkpoint", tip],
            None,
            &[],
        ) {
            eprintln!("memory checkpoint {seq} for {repo_id}: {err:?}");
            book.checkpointing.store(false, Ordering::SeqCst);
            return;
        }
        let service = self.clone();
        let repo_id = repo_id.to_string();
        std::thread::spawn(move || {
            let result = service.write_checkpoint(&repo_id, seq);
            if let Err(err) = result {
                eprintln!("memory checkpoint {seq} for {repo_id}: {err:?}");
            }
            if let Ok(book) = service.book(&repo_id) {
                book.checkpointing.store(false, Ordering::SeqCst);
            }
        });
    }

    fn write_checkpoint(&self, repo_id: &str, seq: u64) -> Result<(), Fail> {
        let repo = self.repo(repo_id)?;
        let path = repo.scratch.join(format!("checkpoint-{seq:020}.bundle"));
        let uploaded = (|| {
            let _ = fs::remove_file(&path);
            git_ok(
                &repo,
                &[
                    "bundle",
                    "create",
                    &path.to_string_lossy(),
                    "refs/cadri/checkpoint",
                ],
                None,
                &[],
            )?;
            let bytes = fs::read(&path).map_err(|err| Fail::git(err.to_string()))?;
            {
                let mut left = self
                    .inner
                    .checkpoint_failures
                    .lock()
                    .unwrap_or_else(|err| err.into_inner());
                if *left > 0 {
                    *left -= 1;
                    return Err(Fail::git("checkpoint upload failed"));
                }
            }
            // `Exists` means an earlier attempt or another process stored it.
            self.inner
                .durable
                .create(repo_id, &checkpoint_key(seq), &bytes)
                .map(|_| ())
        })();
        let _ = fs::remove_file(&path);
        uploaded?;
        let book = self.book(repo_id)?;
        let mut writer = book.writer.lock().unwrap_or_else(|err| err.into_inner());
        if seq > writer.checkpoint_seq {
            writer.checkpoint_seq = seq;
            writer.log_bytes_since_checkpoint = 0;
        }
        Ok(())
    }

    fn log_entry_holds(&self, repo_id: &str, key: &str, sha: &str) -> Result<bool, Fail> {
        let Some(bytes) = self.inner.durable.get(repo_id, key)? else {
            return Ok(false);
        };
        let header = parse_bundle_header(&bytes)?;
        Ok(matches!(header.refs.as_slice(), [(tip, _)] if tip == sha))
    }

    fn take_log_failure(&self) -> bool {
        let mut left = self.inner.log_failures.lock().expect("log failures");
        if *left > 0 {
            *left -= 1;
            true
        } else {
            false
        }
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
        match &self.inner.backend {
            Backend::Directory(store) => fs::read(obj_path(store, repo_id, sha))
                .map_err(|_| Fail::git(format!("memory bundle {sha} is missing"))),
            Backend::S3(books) => books.get_object(repo_id, sha).map_err(s3_fail),
        }
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

type Made = Vec<Result<Option<String>, ApiError>>;

/// Builds one commit per edit, each on top of the last. Returns the snapshot after
/// the last commit (`None` if nothing changed) and each edit's commit or error.
fn build_batch(
    repo: &Repo,
    start: &Snapshot,
    requests: &[EditRequest],
) -> (Option<Snapshot>, Made) {
    let mut next: Option<Snapshot> = None;
    let mut made = Vec::with_capacity(requests.len());
    let mut builder = TreeBuilder::new(repo);
    let texts = requests
        .iter()
        .flat_map(|request| &request.changes)
        .filter_map(|change| match change {
            Change::Write { path, content } if check_content(path, content).is_ok() => {
                Some(content.as_str())
            }
            _ => None,
        });
    // Each edit hashes its own texts if this fails.
    let _ = builder.hash_texts(texts);
    for request in requests {
        match apply_edit(repo, &mut builder, next.as_ref().unwrap_or(start), request) {
            Ok(Some(snapshot)) => {
                made.push(Ok(snapshot.tip.clone()));
                next = Some(snapshot);
            }
            Ok(None) => made.push(Ok(None)),
            Err(err) => made.push(Err(into_api(err))),
        }
    }
    (next, made)
}

fn answer(made: Made, snapshot: &Arc<Snapshot>) -> Vec<Result<EditOutcome, ApiError>> {
    made.into_iter()
        .map(|result| {
            result.map(|commit| EditOutcome {
                commit,
                snapshot: Arc::clone(snapshot),
            })
        })
        .collect()
}

/// The batch was not published: edits that were rejected keep their own error,
/// every other edit gets `err`.
fn fail_committed(made: Made, err: ApiError) -> Vec<Result<EditOutcome, ApiError>> {
    made.into_iter()
        .map(|result| result.and(Err(err.clone())))
        .collect()
}

/// The commit for one edit on top of `current`, as the snapshot after it. `None`
/// when the edit changes nothing.
fn apply_edit(
    repo: &Repo,
    builder: &mut TreeBuilder,
    current: &Snapshot,
    request: &EditRequest,
) -> Result<Option<Snapshot>, Fail> {
    let message = clean_message(&request.message)?;
    if let Some(base) = &request.base {
        check_base(repo, current.tip.as_deref(), base, &request.changes)?;
    }
    let before = current_sources(current);
    let mut files = before.clone();
    apply_changes(&mut files, &request.changes)?;
    // Writing what a file already holds is not a change.
    for (path, source) in files.iter_mut() {
        if let (Source::Text(text), Some(Source::Blob(blob))) = (&*source, before.get(path)) {
            if current.blobs.get(blob).is_some_and(|old| **old == **text) {
                *source = Source::Blob(blob.clone());
            }
        }
    }
    if files == before {
        return Ok(None);
    }
    let parent = current.tip.as_deref();
    let built = builder.build(parent, &before, &files)?;
    let author = match &request.agent {
        Some(agent) => CommitAuthor::Robot {
            agent,
            label: &request.label,
        },
        None => CommitAuthor::Human {
            user_id: &request.user_id,
            label: &request.label,
        },
    };
    let sha = commit_tree(
        repo,
        &built.tree,
        parent,
        &message,
        &author,
        request.session_id.as_deref(),
    )?;
    builder.committed(&sha);
    snapshot_after(current, &sha, &built.files, built.new_blobs).map(Some)
}

/// An edit made against `base` may not touch a path that changed between `base`
/// and `tip`.
fn check_base(repo: &Repo, tip: Option<&str>, base: &str, changes: &[Change]) -> Result<(), Fail> {
    if tip == Some(base) {
        return Ok(());
    }
    let unknown = || Fail::api(StatusCode::CONFLICT, "unknown base");
    let Some(tip) = tip else {
        return Err(unknown());
    };
    if !is_hex(base, 40) || !is_ancestor(repo, base, tip)? {
        return Err(unknown());
    }
    let out = git_bytes(
        repo,
        &["diff-tree", "-r", "--name-only", "-z", base, tip],
        None,
        &[],
    )?;
    let changed: std::collections::HashSet<&[u8]> = out
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .collect();
    let mut paths: Vec<String> = changes
        .iter()
        .flat_map(Change::paths)
        .filter(|path| changed.contains(path.as_bytes()))
        .map(str::to_string)
        .collect();
    paths.sort();
    paths.dedup();
    if paths.is_empty() {
        Ok(())
    } else {
        Err(Fail::Api(ApiError::conflict(paths)))
    }
}

fn loaded(book: &Book) -> Result<Arc<Snapshot>, Fail> {
    book.current()
        .ok_or_else(|| Fail::git("memory snapshot was not loaded"))
}

/// A durable store that cannot be loaded as written. Reported, never repaired.
fn broken(message: impl Into<String>) -> Fail {
    let message = message.into();
    eprintln!("memory store: {message}");
    Fail::api(StatusCode::INTERNAL_SERVER_ERROR, message)
}

/// The objects added since `old`, as a bundle whose one ref, `refs/cadri/stage`,
/// is `sha`. Leaves the stage ref set.
fn log_bundle(repo: &Repo, old: Option<&str>, sha: &str) -> Result<Vec<u8>, Fail> {
    git_ok(repo, &["update-ref", "refs/cadri/stage", sha], None, &[])?;
    let path = repo.scratch.join(format!("{sha}.bundle"));
    let _ = fs::remove_file(&path);
    let path_arg = path.to_string_lossy();
    let exclude = old.map(|old| format!("^{old}"));
    let mut args = vec!["bundle", "create", &path_arg, "refs/cadri/stage"];
    if let Some(exclude) = &exclude {
        args.push(exclude);
    }
    let created = git_ok(repo, &args, None, &[]);
    let bytes = created.and_then(|()| fs::read(&path).map_err(|err| Fail::git(err.to_string())));
    let _ = fs::remove_file(&path);
    bytes
}

/// Fetches one bundle into `main`. Its prerequisites must be met by `tip` (none
/// for an empty book), and afterwards `main` must be its single ref. Returns the
/// new tip.
fn apply_bundle(repo: &Repo, bytes: &[u8], name: &str, tip: Option<&str>) -> Result<String, Fail> {
    let header = parse_bundle_header(bytes)?;
    let [(sha, refname)] = header.refs.as_slice() else {
        return Err(broken(format!("memory {name} must hold exactly one ref")));
    };
    // A merge pushed on top of `tip` can also list older commits as boundaries;
    // `tip` itself is always one of them.
    let follows = match tip {
        None => header.prerequisites.is_empty(),
        Some(tip) => header.prerequisites.iter().any(|needed| needed == tip),
    };
    if !follows {
        return Err(broken(format!(
            "memory {name} does not follow tip {}",
            tip.unwrap_or("(empty)")
        )));
    }
    let path = repo.scratch.join(format!("{name}.bundle"));
    fs::write(&path, bytes).map_err(|err| Fail::git(err.to_string()))?;
    let fetched = git_ok(
        repo,
        &[
            "fetch",
            "--quiet",
            "--no-write-fetch-head",
            &path.to_string_lossy(),
            &format!("+{refname}:refs/heads/main"),
        ],
        None,
        &[],
    );
    let _ = fs::remove_file(&path);
    fetched?;
    if local_head(repo)?.as_deref() != Some(sha.as_str()) {
        return Err(broken(format!("memory {name} did not move main to {sha}")));
    }
    Ok(sha.clone())
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
