//! Two engines, one book, one object store.
//!
//! Each engine has its own git directory and its own SQLite connection. They
//! share the store, which is either the directory of lock files or the S3
//! stand-in. A publish waits at [`PublishGate`] so both have finished
//! validating before either compare-and-swap runs.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use serde_json::{json, Value};

use cadri_core::auth::NoAuth;
use cadri_core::db;
use cadri_core::memory::testing::{assemble, FakeS3, MemoryService, PublishGate, RefFault, S3Books};

#[tokio::test(flavor = "multi_thread")]
async fn two_directory_servers_race_a_git_push_and_the_loser_retries() {
    let root = temp();
    let store = root.join("store");
    let left = MemoryService::open_at(
        root.join("left/repos"),
        store.clone(),
        root.join("left/scratch"),
    );
    let right = MemoryService::open_at(root.join("right/repos"), store, root.join("right/scratch"));
    race_git_pushes(root, left, right, None).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn two_s3_servers_race_a_git_push_and_the_loser_retries() {
    let root = temp();
    let fake = FakeS3::start();
    let client = S3Books::test_client(&fake.endpoint);
    let left = MemoryService::open_s3(
        root.join("left"),
        None,
        false,
        S3Books::from_client("memories", client.clone()),
    );
    let right = MemoryService::open_s3(
        root.join("right"),
        None,
        false,
        S3Books::from_client("memories", client),
    );
    race_git_pushes(root, left, right, Some(&fake)).await;
    assert_eq!(fake.ref_conflicts(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn two_directory_servers_race_a_ui_commit_and_both_land() {
    let root = temp();
    let store = root.join("store");
    let left = MemoryService::open_at(
        root.join("left/repos"),
        store.clone(),
        root.join("left/scratch"),
    );
    let right = MemoryService::open_at(root.join("right/repos"), store, root.join("right/scratch"));
    race_ui_commits(root, left, right, None).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn two_s3_servers_race_a_ui_commit_and_both_land() {
    let root = temp();
    let fake = FakeS3::start();
    let client = S3Books::test_client(&fake.endpoint);
    let left = MemoryService::open_s3(
        root.join("left"),
        None,
        false,
        S3Books::from_client("memories", client.clone()),
    );
    let right = MemoryService::open_s3(
        root.join("right"),
        None,
        false,
        S3Books::from_client("memories", client),
    );
    race_ui_commits(root, left, right, Some(&fake)).await;
    assert_eq!(fake.ref_conflicts(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn s3_crash_before_the_ref_swap_publishes_once_on_retry() {
    let root = temp();
    let (fake, addr, _peer) = s3_pair(&root).await;
    let config = git_config(&root);
    let repo = root.join("agent");
    let url = format!("http://{addr}/git/team.git");
    seed(&repo, &config, &url);

    let sha = commit_file(&repo, &config, "note.md", "after\n", "edit");
    fake.fail_next_ref_put(RefFault::Refuse);
    let failed = git(&repo, &config, &["push", &url, "HEAD:refs/heads/main"]);
    assert!(!failed.status.success(), "{}", text(&failed.stderr));
    assert!(
        fake.keys()
            .iter()
            .any(|key| key.ends_with(&format!("/objects/{sha}"))),
        "the bundle was stored before the ref swap failed: {:?}",
        fake.keys()
    );
    assert_ne!(remote_tip(&url, &config, &root), sha);

    let again = git(&repo, &config, &["push", &url, "HEAD:refs/heads/main"]);
    assert!(again.status.success(), "{}", text(&again.stderr));
    assert_eq!(remote_tip(&url, &config, &root), sha);
    let objects = fake
        .keys()
        .into_iter()
        .filter(|key| key.contains("/objects/"))
        .count();
    assert_eq!(objects, 2, "retry must not write a second bundle for {sha}");
    let _ = fs::remove_dir_all(root);
}

#[tokio::test(flavor = "multi_thread")]
async fn s3_crash_after_the_ref_swap_is_already_published() {
    let root = temp();
    let (fake, addr, peer) = s3_pair(&root).await;
    let config = git_config(&root);
    let repo = root.join("agent");
    let url = format!("http://{addr}/git/team.git");
    seed(&repo, &config, &url);

    let sha = commit_file(&repo, &config, "note.md", "after\n", "edit");
    fake.fail_next_ref_put(RefFault::StoreThenFail);
    let failed = git(&repo, &config, &["push", &url, "HEAD:refs/heads/main"]);
    assert!(!failed.status.success(), "{}", text(&failed.stderr));

    let body = get_json(&peer, "/api/memories");
    assert_eq!(body["commits"][0]["id"], sha);
    assert_eq!(file_map(&body)["note.md"], "after\n");

    let again = git(&repo, &config, &["push", &url, "HEAD:refs/heads/main"]);
    assert!(again.status.success(), "{}", text(&again.stderr));
    let after = get_json(&peer, "/api/memories");
    assert_eq!(after["commits"].as_array().unwrap().len(), 2);
    assert_eq!(after["commits"][0]["id"], sha);
    let _ = fs::remove_dir_all(root);
}

async fn race_git_pushes(
    root: PathBuf,
    left: MemoryService,
    right: MemoryService,
    fake: Option<&FakeS3>,
) {
    let gate = PublishGate::new();
    left.arm_publish_gate(gate.clone());
    right.arm_publish_gate(gate.clone());
    let (left_addr, right_addr) = serve_pair(&root, left, right).await;
    let config = git_config(&root);
    let origin = root.join("origin");
    let left_url = format!("http://{left_addr}/git/team.git");
    let right_url = format!("http://{right_addr}/git/team.git");
    seed(&origin, &config, &left_url);

    let right_agent = root.join("right-agent");
    assert_ok(git(&right_agent, &config, &["clone", &right_url, "."]));
    let left_sha = commit_file(
        &origin,
        &config,
        "left.md",
        "from the left server\n",
        "left",
    );
    let right_sha = commit_file(
        &right_agent,
        &config,
        "right.md",
        "from the right server\n",
        "right",
    );

    gate.arm(2);
    let left_push = push_in_thread(origin.clone(), config.clone(), left_url.clone());
    let right_push = push_in_thread(right_agent.clone(), config.clone(), right_url.clone());
    let left_result = left_push.join().unwrap();
    let right_result = right_push.join().unwrap();
    let successes = [left_result.success, right_result.success]
        .into_iter()
        .filter(|ok| *ok)
        .count();
    assert_eq!(
        successes, 1,
        "left: {}\nright: {}",
        left_result.detail, right_result.detail
    );
    assert_eq!(
        gate.conflicts(),
        1,
        "both servers must lose one compare-and-swap"
    );
    if let Some(fake) = fake {
        assert_eq!(fake.ref_conflicts(), 1);
    }
    let failed_is_left = !left_result.success;
    assert!(
        (if failed_is_left {
            &left_result
        } else {
            &right_result
        })
        .detail
        .contains("fast-forward"),
        "left: {}\nright: {}",
        left_result.detail,
        right_result.detail
    );

    let (loser_dir, loser_url, winner_sha) = if failed_is_left {
        (origin, left_url, right_sha)
    } else {
        (right_agent, right_url, left_sha)
    };
    let fetched = fetch_main(&loser_dir, &config, &loser_url);
    assert_eq!(fetched, winner_sha);
    assert_ok(git(&loser_dir, &config, &["rebase", &fetched]));
    assert_ok(git(
        &loser_dir,
        &config,
        &["push", &loser_url, "HEAD:refs/heads/main"],
    ));
    let tip = rev_parse(&loser_dir, &config, "HEAD");

    let body = get_json(&right_addr, "/api/memories");
    let ids: Vec<&str> = body["commits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|commit| commit["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids[0], tip);
    assert!(ids.contains(&winner_sha.as_str()), "{ids:?}");
    let files = file_map(&body);
    assert_eq!(files["left.md"], "from the left server\n");
    assert_eq!(files["right.md"], "from the right server\n");
    let _ = fs::remove_dir_all(root);
}

async fn race_ui_commits(
    root: PathBuf,
    left: MemoryService,
    right: MemoryService,
    fake: Option<&FakeS3>,
) {
    let gate = PublishGate::new();
    left.arm_publish_gate(gate.clone());
    right.arm_publish_gate(gate.clone());
    let (left_addr, right_addr) = serve_pair(&root, left, right).await;
    gate.arm(2);
    let left_post = post_in_thread(left_addr, "left.md", "from the left server\n", "Add left");
    let right_post = post_in_thread(
        right_addr,
        "right.md",
        "from the right server\n",
        "Add right",
    );
    let left_status = left_post.join().unwrap();
    let right_status = right_post.join().unwrap();
    assert_eq!(left_status, 200, "left UI commit failed");
    assert_eq!(right_status, 200, "right UI commit failed");
    assert_eq!(gate.conflicts(), 1);
    if let Some(fake) = fake {
        assert_eq!(fake.ref_conflicts(), 1);
    }
    let body = get_json(&right_addr, "/api/memories");
    let files = file_map(&body);
    assert_eq!(files["left.md"], "from the left server\n");
    assert_eq!(files["right.md"], "from the right server\n");
    assert_eq!(body["commits"].as_array().unwrap().len(), 2);
    let _ = fs::remove_dir_all(root);
}

async fn s3_pair(root: &Path) -> (FakeS3, std::net::SocketAddr, std::net::SocketAddr) {
    let fake = FakeS3::start();
    let client = S3Books::test_client(&fake.endpoint);
    let left = MemoryService::open_s3(
        root.join("left"),
        None,
        false,
        S3Books::from_client("memories", client.clone()),
    );
    let right = MemoryService::open_s3(
        root.join("right"),
        None,
        false,
        S3Books::from_client("memories", client),
    );
    let (left_addr, right_addr) = serve_pair(root, left, right).await;
    (fake, left_addr, right_addr)
}

async fn serve_pair(
    root: &Path,
    left: MemoryService,
    right: MemoryService,
) -> (std::net::SocketAddr, std::net::SocketAddr) {
    let db_path = root.join("transcripts.db");
    let left_addr = listen(assemble(
        db::open(&db_path).unwrap(),
        Arc::new(NoAuth),
        None,
        left,
    ))
    .await;
    let right_addr = listen(assemble(
        db::open(&db_path).unwrap(),
        Arc::new(NoAuth),
        None,
        right,
    ))
    .await;
    (left_addr, right_addr)
}

async fn listen(app: Router) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

fn seed(dir: &Path, config: &Path, url: &str) {
    assert_ok(git(dir, config, &["init", "-b", "main"]));
    commit_file(dir, config, "base.md", "base\n", "base");
    assert_ok(git(dir, config, &["push", url, "HEAD:refs/heads/main"]));
}

fn commit_file(dir: &Path, config: &Path, name: &str, content: &str, message: &str) -> String {
    fs::write(dir.join(name), content).unwrap();
    assert_ok(git(dir, config, &["add", name]));
    let message_path = dir.join("COMMIT_EDITMSG_CADRI");
    fs::write(
        &message_path,
        format!(
            "{message}\n\nCadri-Author-Kind: human\nCadri-Author-Id: default\nCadri-Author-Label: default\n"
        ),
    )
    .unwrap();
    assert_ok(git(dir, config, &["commit", "-F", "COMMIT_EDITMSG_CADRI"]));
    rev_parse(dir, config, "HEAD")
}

fn fetch_main(dir: &Path, config: &Path, url: &str) -> String {
    assert_ok(git(
        dir,
        config,
        &["fetch", url, "+refs/heads/main:refs/cadri/main"],
    ));
    rev_parse(dir, config, "refs/cadri/main")
}

fn remote_tip(url: &str, config: &Path, root: &Path) -> String {
    let listed = git(root, config, &["ls-remote", url, "refs/heads/main"]);
    assert_ok(listed.clone());
    text(&listed.stdout)
        .split_whitespace()
        .next()
        .unwrap()
        .to_string()
}

struct PushResult {
    success: bool,
    detail: String,
}

fn push_in_thread(
    dir: PathBuf,
    config: PathBuf,
    url: String,
) -> std::thread::JoinHandle<PushResult> {
    std::thread::spawn(move || {
        let output = git(&dir, &config, &["push", &url, "HEAD:refs/heads/main"]);
        PushResult {
            success: output.status.success(),
            detail: format!("{}\n{}", text(&output.stdout), text(&output.stderr)),
        }
    })
}

fn post_in_thread(
    addr: std::net::SocketAddr,
    path: &str,
    content: &str,
    message: &str,
) -> std::thread::JoinHandle<u16> {
    let path = path.to_string();
    let content = content.to_string();
    let message = message.to_string();
    std::thread::spawn(move || {
        let body = serde_json::to_vec(&json!({
            "message": message,
            "changes": [{ "op": "write", "path": path, "content": content }]
        }))
        .unwrap();
        let (status, _) = http(&addr, "POST", "/api/memories/commits", Some(&body));
        status
    })
}

fn get_json(addr: &std::net::SocketAddr, path: &str) -> Value {
    let (status, body) = http(addr, "GET", path, None);
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

fn http(
    addr: &std::net::SocketAddr,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
) -> (u16, Vec<u8>) {
    let body = body.unwrap_or(b"");
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let header = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
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

fn git_config(root: &Path) -> PathBuf {
    let config = root.join("gitconfig");
    fs::write(
        &config,
        "[user]\n\tname = Default\n\temail = default@cadri.local\n",
    )
    .unwrap();
    config
}

struct GitOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl Clone for GitOutput {
    fn clone(&self) -> Self {
        Self {
            status: self.status,
            stdout: self.stdout.clone(),
            stderr: self.stderr.clone(),
        }
    }
}

fn git(dir: &Path, config: &Path, args: &[&str]) -> GitOutput {
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::create_dir_all(dir).ok();
    let output = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .args(args)
        .output()
        .unwrap();
    GitOutput {
        status: output.status,
        stdout: output.stdout,
        stderr: output.stderr,
    }
}

fn assert_ok(output: GitOutput) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        text(&output.stdout),
        text(&output.stderr)
    );
}

fn rev_parse(dir: &Path, config: &Path, rev: &str) -> String {
    let output = git(dir, config, &["rev-parse", rev]);
    assert_ok(output.clone());
    text(&output.stdout).trim().to_string()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn temp() -> PathBuf {
    static TEMPS: AtomicU64 = AtomicU64::new(0);
    let n = TEMPS.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "cadri-race-{}-{}-{}",
        std::process::id(),
        n,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}
