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
    assert_ok(git(
        &clone,
        &config,
        &["clone", &url, "."],
        "org-a",
        "ada",
    ));
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
    assert_ok(git(&origin, &config, &["init", "-b", "main"], "org-a", "ada"));
    commit_file(&origin, &config, "first.md", "one\n", "first", "org-a", "ada");
    commit_file(&origin, &config, "second.md", "two\n", "second", "org-a", "ada");
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
    assert_ok(git(
        &spy,
        &b_config,
        &["clone", &url, "."],
        "org-b",
        "bob",
    ));
    assert!(!spy.join("secret.md").exists());
    let _ = fs::remove_dir_all(root);
}

async fn serve(root: &Path) -> std::net::SocketAddr {
    let memories = MemoryService::open_local(root.join("memories"), None, false);
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
