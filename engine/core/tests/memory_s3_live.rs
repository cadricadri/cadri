//! Live check against a real S3 bucket. Helpers first; the ignored test is at
//! the bottom. Uses [`cadri_core::memory::testing`] for engine internals.
//!
//! Set `CADRI_MEMORIES_BUCKET` and `AWS_REGION`, with credentials the bucket
//! accepts. Two engines share that bucket. Git clients create many books, push
//! repeatedly, and sometimes diverge. A lost push is `non-fast-forward`. The
//! client fetches, rebases, resolves a content conflict when there is one, and
//! pushes again. The other engine reads the tip back from S3.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::Arc;
use std::time::Duration;

use axum::http::HeaderMap;
use serde_json::Value;

use cadri_core::auth::{AuthFuture, Authenticated, Authenticator, Identity};
use cadri_core::db;
use cadri_core::memory::testing::{assemble, memory_repo_id, MemoryService, S3Books};

const NOTES: usize = 5;

#[derive(Clone, Copy)]
enum Conflict {
    Disjoint,
    SameFile,
}

struct Book {
    org: String,
    user: String,
    scope: &'static str,
    conflict: Option<Conflict>,
}

impl Book {
    fn label(&self) -> String {
        format!("{}/{}:{}", self.org, self.scope, self.user)
    }

    fn git_name(&self) -> &'static str {
        if self.scope == "user" {
            "user.git"
        } else {
            "team.git"
        }
    }

    fn repo_user(&self) -> &str {
        if self.scope == "user" {
            &self.user
        } else {
            ""
        }
    }
}

fn books() -> Vec<Book> {
    let mut books = Vec::new();
    for index in 0..8 {
        let conflict = match index {
            0 | 1 => Some(Conflict::Disjoint),
            2 | 3 => Some(Conflict::SameFile),
            _ => None,
        };
        books.push(Book {
            org: format!("org-{index:02}"),
            user: "default".into(),
            scope: "team",
            conflict,
        });
    }
    for index in 0..4 {
        books.push(Book {
            org: format!("org-{index:02}"),
            user: format!("ada-{index}"),
            scope: "user",
            conflict: None,
        });
    }
    books
}

fn fill_book(root: &Path, left: std::net::SocketAddr, right: std::net::SocketAddr, book: &Book) {
    let client = Client::new(root, book, "writer");
    let first = url(left, book);
    git_ok(&client, &["init", "-b", "main"]);
    commit(&client, "base.md", "base\n", "base");
    git_ok(&client, &["push", &first, "HEAD:refs/heads/main"]);
    for index in 0..NOTES {
        let server = if index % 2 == 0 { right } else { left };
        commit(
            &client,
            &format!("note-{index}.md"),
            &format!("note {index}\n"),
            &format!("note {index}"),
        );
        git_ok(
            &client,
            &["push", &url(server, book), "HEAD:refs/heads/main"],
        );
    }
}

fn race_book(root: &Path, left: std::net::SocketAddr, right: std::net::SocketAddr, book: &Book) {
    let origin = Client::new(root, book, "origin");
    git_ok(&origin, &["clone", &url(left, book), "."]);
    let other = Client::new(root, book, "other");
    git_ok(&other, &["clone", &url(right, book), "."]);

    let (left_path, left_body, right_path, right_body) = match book.conflict.unwrap() {
        Conflict::Disjoint => ("left.md", "from the left\n", "right.md", "from the right\n"),
        Conflict::SameFile => ("shared.md", "left\n", "shared.md", "right\n"),
    };
    commit(&origin, left_path, left_body, "left");
    commit(&other, right_path, right_body, "right");

    let origin_push = push_later(origin.clone_paths(), url(left, book));
    let other_push = push_later(other.clone_paths(), url(right, book));
    let origin_result = origin_push.join().unwrap();
    let other_result = other_push.join().unwrap();
    let successes = usize::from(origin_result.success) + usize::from(other_result.success);
    assert_eq!(
        successes,
        1,
        "{}\norigin: {}\nother: {}",
        book.label(),
        origin_result.detail,
        other_result.detail
    );
    let (loser, winner_addr) = if origin_result.success {
        assert!(
            other_result.detail.contains("fast-forward"),
            "{}",
            other_result.detail
        );
        (&other, left)
    } else {
        assert!(
            origin_result.detail.contains("fast-forward"),
            "{}",
            origin_result.detail
        );
        (&origin, right)
    };

    let fetched = fetch_main(loser, &url(winner_addr, book));
    let rebase = git(loser, &["rebase", &fetched]);
    if !rebase.status.success() {
        let shared = loser.dir.join("shared.md");
        let text = fs::read_to_string(&shared).unwrap_or_default();
        assert!(
            text.contains("<<<<<<<"),
            "{} rebase failed without a content conflict:\n{}\n{}",
            book.label(),
            text_of(&rebase.stdout),
            text_of(&rebase.stderr)
        );
        fs::write(&shared, "left\nright\n").unwrap();
        git_ok(loser, &["add", "shared.md"]);
        git_ok(loser, &["rebase", "--continue"]);
    }
    git_ok(
        loser,
        &["push", &url(winner_addr, book), "HEAD:refs/heads/main"],
    );
}

struct Client {
    dir: PathBuf,
    config: PathBuf,
    org: String,
    user: String,
    book: String,
}

impl Client {
    fn new(root: &Path, book: &Book, name: &str) -> Self {
        let dir = root.join(format!("{}-{}-{}-{name}", book.org, book.scope, book.user));
        fs::create_dir_all(&dir).unwrap();
        let config = root.join(format!(
            "{}-{}-{}-{name}.gitconfig",
            book.org, book.scope, book.user
        ));
        fs::write(
            &config,
            "[user]\n\tname = Default\n\temail = default@cadri.local\n",
        )
        .unwrap();
        Self {
            dir,
            config,
            org: book.org.clone(),
            user: book.user.clone(),
            book: book.git_name().to_string(),
        }
    }

    fn clone_paths(&self) -> Self {
        Self {
            dir: self.dir.clone(),
            config: self.config.clone(),
            org: self.org.clone(),
            user: self.user.clone(),
            book: self.book.clone(),
        }
    }
}

fn commit(client: &Client, name: &str, content: &str, message: &str) {
    let path = client.dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, content).unwrap();
    git_ok(client, &["add", name]);
    let message_path = client.dir.join("COMMIT_EDITMSG_CADRI");
    fs::write(
        &message_path,
        format!(
            "{message}\n\nCadri-Author-Kind: human\nCadri-Author-Id: {}\nCadri-Author-Label: {}\n",
            client.user, client.user
        ),
    )
    .unwrap();
    git_ok(client, &["commit", "-F", "COMMIT_EDITMSG_CADRI"]);
}

fn fetch_main(client: &Client, url: &str) -> String {
    git_ok(client, &["fetch", url, "+refs/heads/main:refs/cadri/main"]);
    rev_parse(client, "refs/cadri/main")
}

fn url(addr: std::net::SocketAddr, book: &Book) -> String {
    format!("http://{addr}/git/{}", book.git_name())
}

fn push_later(client: Client, url: String) -> std::thread::JoinHandle<PushResult> {
    std::thread::spawn(move || {
        let output = git(&client, &["push", &url, "HEAD:refs/heads/main"]);
        PushResult {
            success: output.status.success(),
            detail: format!("{}\n{}", text_of(&output.stdout), text_of(&output.stderr)),
        }
    })
}

struct PushResult {
    success: bool,
    detail: String,
}

fn store(addr: std::net::SocketAddr, book: &Book) -> Value {
    let path = if book.scope == "user" {
        "/api/memories?scope=user"
    } else {
        "/api/memories"
    };
    json_at(addr, book, path)
}

fn log(addr: std::net::SocketAddr, book: &Book) -> Value {
    let path = if book.scope == "user" {
        "/api/memories/log?scope=user"
    } else {
        "/api/memories/log"
    };
    json_at(addr, book, path)
}

fn json_at(addr: std::net::SocketAddr, book: &Book, path: &str) -> Value {
    let (status, body) = http(addr, "GET", path, &book.org, &book.user, None);
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice(&body).unwrap()
}

fn file_map(store: &Value) -> std::collections::HashMap<String, String> {
    store["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| {
            (
                file["path"].as_str().unwrap().to_string(),
                file["content"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn ls_remote(addr: std::net::SocketAddr, book: &Book) -> String {
    let client = Client {
        dir: std::env::temp_dir(),
        config: std::env::temp_dir().join(format!("cadri-ls-{}-{}", book.org, book.user)),
        org: book.org.clone(),
        user: book.user.clone(),
        book: book.git_name().to_string(),
    };
    if !client.config.exists() {
        fs::write(&client.config, "[user]\n\tname = Default\n").unwrap();
    }
    let output = git(&client, &["ls-remote", &url(addr, book), "refs/heads/main"]);
    git_ok_output(&output);
    text_of(&output.stdout)
        .split_whitespace()
        .next()
        .unwrap()
        .to_string()
}

fn list_keys(bucket: &str, prefix: &str) -> Vec<String> {
    let output = Command::new("aws")
        .args([
            "s3api",
            "list-objects-v2",
            "--bucket",
            bucket,
            "--prefix",
            prefix,
            "--query",
            "Contents[].Key",
            "--output",
            "text",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    text_of(&output.stdout)
        .split_whitespace()
        .filter(|key| !key.is_empty() && *key != "None")
        .map(str::to_string)
        .collect()
}

async fn serve_pair(
    db_path: &Path,
    left: MemoryService,
    right: MemoryService,
) -> (std::net::SocketAddr, std::net::SocketAddr) {
    let left_addr = listen(assemble(
        db::open(db_path).unwrap(),
        Arc::new(OrgHeader),
        None,
        left,
    ))
    .await;
    let right_addr = listen(assemble(
        db::open(db_path).unwrap(),
        Arc::new(OrgHeader),
        None,
        right,
    ))
    .await;
    (left_addr, right_addr)
}

async fn listen(app: axum::Router) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

struct OrgHeader;

impl Authenticator for OrgHeader {
    fn mode(&self) -> &'static str {
        "test"
    }

    fn authenticate<'a>(&'a self, headers: &'a HeaderMap) -> AuthFuture<'a> {
        Box::pin(async move {
            let org = header_value(headers, "x-cadri-org").unwrap_or_else(|| "default".into());
            let user = header_value(headers, "x-cadri-user").unwrap_or_else(|| "default".into());
            Ok(Authenticated::from(Identity {
                user_id: user,
                email: None,
                org_id: org,
                org_name: None,
            }))
        })
    }
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn http(
    addr: std::net::SocketAddr,
    method: &str,
    path: &str,
    org: &str,
    user: &str,
    body: Option<&[u8]>,
) -> (u16, Vec<u8>) {
    let body = body.unwrap_or(b"");
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .unwrap();
    let header = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nx-cadri-org: {org}\r\nx-cadri-user: {user}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let text = String::from_utf8_lossy(&raw);
    let (head, payload) = text.split_once("\r\n\r\n").unwrap_or_else(|| {
        panic!("short HTTP response: {text}");
    });
    let status: u16 = head
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    (status, payload.as_bytes().to_vec())
}

struct GitOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn git(client: &Client, args: &[&str]) -> GitOutput {
    fs::create_dir_all(&client.dir).ok();
    let org = format!("http.extraHeader=x-cadri-org:{}", client.org);
    let user = format!("http.extraHeader=x-cadri-user:{}", client.user);
    let output = Command::new("git")
        .current_dir(&client.dir)
        .env("GIT_CONFIG_GLOBAL", &client.config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .arg("-c")
        .arg(&org)
        .arg("-c")
        .arg(&user)
        .args(args)
        .output()
        .unwrap();
    GitOutput {
        status: output.status,
        stdout: output.stdout,
        stderr: output.stderr,
    }
}

fn git_ok(client: &Client, args: &[&str]) {
    git_ok_output(&git(client, args));
}

fn git_ok_output(output: &GitOutput) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        text_of(&output.stdout),
        text_of(&output.stderr)
    );
}

fn rev_parse(client: &Client, rev: &str) -> String {
    let output = git(client, &["rev-parse", rev]);
    git_ok_output(&output);
    text_of(&output.stdout).trim().to_string()
}

fn text_of(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn temp_dir() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "cadri-s3-live-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "set CADRI_MEMORIES_BUCKET, AWS_REGION, and credentials"]
async fn live_s3_many_books_and_conflicting_pushes() {
    let bucket = std::env::var("CADRI_MEMORIES_BUCKET").expect("CADRI_MEMORIES_BUCKET");
    let root = temp_dir();
    let db_path = root.join("transcripts.db");
    db::open(&db_path).unwrap();

    let left = MemoryService::open_s3(
        root.join("left"),
        None,
        false,
        S3Books::from_env(bucket.clone()).await.unwrap(),
    );
    let right = MemoryService::open_s3(
        root.join("right"),
        None,
        false,
        S3Books::from_env(bucket.clone()).await.unwrap(),
    );
    let (left_addr, right_addr) = serve_pair(&db_path, left, right).await;
    let books = books();

    std::thread::scope(|scope| {
        for book in &books {
            let root = root.clone();
            scope.spawn(move || fill_book(&root, left_addr, right_addr, book));
        }
    });

    let conflicts: Vec<&Book> = books
        .iter()
        .filter(|book| book.conflict.is_some())
        .collect();
    std::thread::scope(|scope| {
        for book in conflicts {
            let root = root.clone();
            scope.spawn(move || race_book(&root, left_addr, right_addr, book));
        }
    });

    for book in &books {
        let from_left = store(left_addr, book);
        let from_right = store(right_addr, book);
        assert_eq!(from_left, from_right, "{}", book.org);
        let files = file_map(&from_right);
        assert_eq!(files["base.md"], "base\n", "{}", book.label());
        for index in 0..NOTES {
            assert_eq!(
                files[&format!("note-{index}.md")],
                format!("note {index}\n"),
                "{}",
                book.label()
            );
        }
        match book.conflict {
            Some(Conflict::Disjoint) => {
                assert_eq!(files["left.md"], "from the left\n");
                assert_eq!(files["right.md"], "from the right\n");
            }
            Some(Conflict::SameFile) => {
                assert_eq!(files["shared.md"], "left\nright\n");
            }
            None => {}
        }
        let commits = log(right_addr, book)["commits"].as_array().unwrap().clone();
        let expected = NOTES + 1 + usize::from(book.conflict.is_some()) * 2;
        assert_eq!(commits.len(), expected, "{}", book.label());
    }

    for book in &books {
        let repo_id = memory_repo_id(&book.org, book.scope, book.repo_user())
            .unwrap_or_else(|| panic!("repo id for {}", book.label()));
        let prefix = format!("repos/{repo_id}/");
        let keys = list_keys(&bucket, &prefix);
        let objects = keys.iter().filter(|key| key.contains("/objects/")).count();
        assert!(
            keys.iter().any(|key| key.ends_with("/refs/heads/main")),
            "missing ref for {prefix}: {keys:?}"
        );
        let commits = log(right_addr, book)["commits"].as_array().unwrap().len();
        let orphans = usize::from(book.conflict.is_some());
        assert!(
            (commits..=commits + orphans).contains(&objects),
            "{prefix} stored {objects} bundles for {commits} commits"
        );
        let remote = ls_remote(right_addr, book);
        let tip = log(right_addr, book)["commits"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(remote, tip, "{}", book.label());
    }

    let _ = fs::remove_dir_all(root);
}
