//! Smart HTTP coverage the race suite does not exercise: empty-book clone,
//! shallow clone, and a second org that cannot fetch another team's tip.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::http::HeaderMap;
use axum::Router;

use cadri_core::auth::{AuthFuture, Authenticated, Authenticator, Identity};
use cadri_core::db;
use cadri_core::memory::testing::{assemble, MemoryService};

#[tokio::test(flavor = "multi_thread")]
async fn empty_team_book_clones_with_unborn_head() {
    let root = temp();
    let addr = serve(&root).await;
    let config = git_config(&root, "org-a", "ada");
    let clone = root.join("empty-clone");
    let url = format!("http://{addr}/git/team.git");
    assert_ok(git(&clone, &config, &["clone", &url, "."], "org-a", "ada"));
    let listed = git(
        &clone,
        &config,
        &["ls-remote", &url, "refs/heads/main"],
        "org-a",
        "ada",
    );
    assert_ok(listed.clone());
    assert!(
        text(&listed.stdout).trim().is_empty(),
        "empty book has no main tip: {}",
        text(&listed.stdout)
    );
    let _ = fs::remove_dir_all(root);
}

#[tokio::test(flavor = "multi_thread")]
async fn shallow_clone_gets_the_tip_tree() {
    let root = temp();
    let addr = serve(&root).await;
    let config = git_config(&root, "org-a", "ada");
    let origin = root.join("origin");
    let url = format!("http://{addr}/git/team.git");
    assert_ok(git(
        &origin,
        &config,
        &["init", "-b", "main"],
        "org-a",
        "ada",
    ));
    commit_file(
        &origin, &config, "first.md", "one\n", "first", "org-a", "ada",
    );
    commit_file(
        &origin,
        &config,
        "second.md",
        "two\n",
        "second",
        "org-a",
        "ada",
    );
    assert_ok(git(
        &origin,
        &config,
        &["push", &url, "HEAD:refs/heads/main"],
        "org-a",
        "ada",
    ));
    let tip = rev_parse(&origin, &config, "HEAD", "org-a", "ada");

    let shallow = root.join("shallow");
    assert_ok(git(
        &shallow,
        &config,
        &["clone", "--depth", "1", &url, "."],
        "org-a",
        "ada",
    ));
    assert_eq!(rev_parse(&shallow, &config, "HEAD", "org-a", "ada"), tip);
    assert_eq!(
        fs::read_to_string(shallow.join("second.md")).unwrap(),
        "two\n"
    );
    let depth = git(
        &shallow,
        &config,
        &["rev-list", "--count", "HEAD"],
        "org-a",
        "ada",
    );
    assert_ok(depth.clone());
    assert_eq!(text(&depth.stdout).trim(), "1");
    let _ = fs::remove_dir_all(root);
}

#[tokio::test(flavor = "multi_thread")]
async fn another_org_cannot_fetch_this_team_book() {
    let root = temp();
    let addr = serve(&root).await;
    let a_config = git_config(&root, "org-a", "ada");
    let b_config = git_config(&root, "org-b", "bob");
    let origin = root.join("origin");
    let url = format!("http://{addr}/git/team.git");
    assert_ok(git(
        &origin,
        &a_config,
        &["init", "-b", "main"],
        "org-a",
        "ada",
    ));
    let tip = commit_file(
        &origin,
        &a_config,
        "secret.md",
        "org-a only\n",
        "secret",
        "org-a",
        "ada",
    );
    assert_ok(git(
        &origin,
        &a_config,
        &["push", &url, "HEAD:refs/heads/main"],
        "org-a",
        "ada",
    ));

    let a_listed = git(
        &origin,
        &a_config,
        &["ls-remote", &url, "refs/heads/main"],
        "org-a",
        "ada",
    );
    assert_ok(a_listed.clone());
    assert!(
        text(&a_listed.stdout).contains(&tip),
        "owner sees tip: {}",
        text(&a_listed.stdout)
    );

    let b_listed = git(
        &root.join("other"),
        &b_config,
        &["ls-remote", &url, "refs/heads/main"],
        "org-b",
        "bob",
    );
    assert_ok(b_listed.clone());
    assert!(
        text(&b_listed.stdout).trim().is_empty(),
        "other org must not see org-a's tip: {}",
        text(&b_listed.stdout)
    );

    let spy = root.join("spy");
    assert_ok(git(&spy, &b_config, &["clone", &url, "."], "org-b", "bob"));
    assert!(!spy.join("secret.md").exists());
    let _ = fs::remove_dir_all(root);
}

#[tokio::test(flavor = "multi_thread")]
async fn fetch_with_many_local_commits_sends_a_gzip_body_and_succeeds() {
    let root = temp();
    let addr = serve(&root).await;
    let config = git_config(&root, "org-a", "ada");
    let url = format!("http://{addr}/git/team.git");
    let origin = root.join("origin");
    assert_ok(git(
        &origin,
        &config,
        &["init", "-b", "main"],
        "org-a",
        "ada",
    ));
    commit_file(
        &origin, &config, "base.md", "base\n", "base", "org-a", "ada",
    );
    assert_ok(git(
        &origin,
        &config,
        &["push", &url, "HEAD:refs/heads/main"],
        "org-a",
        "ada",
    ));
    let local = root.join("local");
    assert_ok(git(&local, &config, &["clone", &url, "."], "org-a", "ada"));
    for n in 0..120 {
        commit_file(
            &local,
            &config,
            &format!("local-{n}.md"),
            "x\n",
            &format!("local {n}"),
            "org-a",
            "ada",
        );
    }
    let tip = commit_file(
        &origin,
        &config,
        "remote.md",
        "remote\n",
        "remote",
        "org-a",
        "ada",
    );
    assert_ok(git(
        &origin,
        &config,
        &["push", &url, "HEAD:refs/heads/main"],
        "org-a",
        "ada",
    ));

    let output = Command::new("git")
        .current_dir(&local)
        .env("GIT_CONFIG_GLOBAL", &config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_TRACE_CURL", "1")
        .env("GIT_TRACE_CURL_NO_DATA", "1")
        .args(["-c", "http.extraHeader=x-cadri-org:org-a"])
        .args(["-c", "http.extraHeader=x-cadri-user:ada"])
        .args(["fetch", "origin"])
        .output()
        .unwrap();
    let trace = text(&output.stderr);
    assert!(output.status.success(), "fetch failed:\n{trace}");
    assert!(
        trace
            .to_ascii_lowercase()
            .contains("content-encoding: gzip"),
        "git did not gzip the request:\n{trace}"
    );
    assert_eq!(
        rev_parse(&local, &config, "origin/main", "org-a", "ada"),
        tip
    );
    let _ = fs::remove_dir_all(root);
}

#[tokio::test(flavor = "multi_thread")]
async fn gzip_upload_pack_request_matches_the_plain_request() {
    let root = temp();
    let app = serve_router(&root);
    let plain = post_upload_pack(&app, None, upload_pack_body()).await;
    assert_eq!(plain.0, 200);
    let gzipped = post_upload_pack(&app, Some("gzip"), gzip(&upload_pack_body())).await;
    assert_eq!(gzipped.0, 200);
    assert_eq!(gzipped.1, plain.1);
    let identity = post_upload_pack(&app, Some("identity"), upload_pack_body()).await;
    assert_eq!(identity.0, 200);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test(flavor = "multi_thread")]
async fn gzip_body_that_expands_past_the_cap_is_rejected() {
    let root = temp();
    let app = serve_router(&root);
    let bomb = gzip(&vec![0u8; 32 * 1024 * 1024 + 1]);
    assert!(bomb.len() < 1024 * 1024);
    let response = post_upload_pack(&app, Some("gzip"), bomb).await;
    assert_eq!(response.0, 413);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_gzip_body_is_a_bad_request() {
    let root = temp();
    let app = serve_router(&root);
    let response = post_upload_pack(&app, Some("gzip"), b"not gzip at all".to_vec()).await;
    assert_eq!(response.0, 400);
    let mut truncated = gzip(&upload_pack_body());
    truncated.truncate(truncated.len() - 4);
    let response = post_upload_pack(&app, Some("gzip"), truncated).await;
    assert_eq!(response.0, 400);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_content_encoding_is_unsupported() {
    let root = temp();
    let app = serve_router(&root);
    let response = post_upload_pack(&app, Some("br"), upload_pack_body()).await;
    assert_eq!(response.0, 415);
    let _ = fs::remove_dir_all(root);
}

fn upload_pack_body() -> Vec<u8> {
    b"0000".to_vec()
}

fn gzip(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

fn serve_router(root: &Path) -> Router {
    let memories = MemoryService::open_local(root.join("memories"), false);
    assemble(
        db::open(&root.join("transcripts.db")).unwrap(),
        Arc::new(OrgHeader),
        None,
        memories,
    )
}

async fn post_upload_pack(app: &Router, encoding: Option<&str>, body: Vec<u8>) -> (u16, Vec<u8>) {
    use tower::ServiceExt;
    let mut request = axum::http::Request::builder()
        .method("POST")
        .uri("/git/team.git/git-upload-pack")
        .header("x-cadri-org", "org-a")
        .header("x-cadri-user", "ada")
        .header("git-protocol", "version=2")
        .header("content-type", "application/x-git-upload-pack-request");
    if let Some(encoding) = encoding {
        request = request.header("content-encoding", encoding);
    }
    let response = app
        .clone()
        .oneshot(request.body(axum::body::Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, bytes.to_vec())
}

async fn serve(root: &Path) -> std::net::SocketAddr {
    let memories = MemoryService::open_local(root.join("memories"), false);
    let app = assemble(
        db::open(&root.join("transcripts.db")).unwrap(),
        Arc::new(OrgHeader),
        None,
        memories,
    );
    listen(app).await
}

async fn listen(app: Router) -> std::net::SocketAddr {
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

fn commit_file(
    dir: &Path,
    config: &Path,
    name: &str,
    content: &str,
    message: &str,
    org: &str,
    user: &str,
) -> String {
    fs::write(dir.join(name), content).unwrap();
    assert_ok(git(dir, config, &["add", name], org, user));
    let message_path = dir.join("COMMIT_EDITMSG_CADRI");
    fs::write(
        &message_path,
        format!(
            "{message}\n\nCadri-Author-Kind: human\nCadri-Author-Id: {user}\nCadri-Author-Label: {user}\n"
        ),
    )
    .unwrap();
    assert_ok(git(
        dir,
        config,
        &["commit", "-F", "COMMIT_EDITMSG_CADRI"],
        org,
        user,
    ));
    rev_parse(dir, config, "HEAD", org, user)
}

fn rev_parse(dir: &Path, config: &Path, rev: &str, org: &str, user: &str) -> String {
    let output = git(dir, config, &["rev-parse", rev], org, user);
    assert_ok(output.clone());
    text(&output.stdout).trim().to_string()
}

fn git_config(root: &Path, org: &str, user: &str) -> PathBuf {
    let config = root.join(format!("{org}-{user}.gitconfig"));
    fs::write(
        &config,
        format!("[user]\n\tname = {user}\n\temail = {user}@cadri.local\n"),
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

fn git(dir: &Path, config: &Path, args: &[&str], org: &str, user: &str) -> GitOutput {
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::create_dir_all(dir).ok();
    let org_header = format!("http.extraHeader=x-cadri-org:{org}");
    let user_header = format!("http.extraHeader=x-cadri-user:{user}");
    let output = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .arg("-c")
        .arg(&org_header)
        .arg("-c")
        .arg(&user_header)
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

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn temp() -> PathBuf {
    static TEMPS: AtomicU64 = AtomicU64::new(0);
    let n = TEMPS.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "cadri-git-http-{}-{}-{}",
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
