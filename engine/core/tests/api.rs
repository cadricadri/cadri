use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, Method, Request, StatusCode};
use axum::routing::get;
use axum::Router;
use cadri_core::auth::{NoAuth, TokenAuth};
use cadri_core::{db, router, ApiError, AuthFuture, Authenticated, Authenticator, Identity};
use serde_json::{json, Value};
use tower::ServiceExt;

struct OrgHeaderAuth;

impl Authenticator for OrgHeaderAuth {
    fn mode(&self) -> &'static str {
        "test"
    }

    fn authenticate<'a>(&'a self, headers: &'a HeaderMap) -> AuthFuture<'a> {
        Box::pin(async move {
            let org = headers
                .get("x-test-org")
                .and_then(|value| value.to_str().ok())
                .ok_or_else(ApiError::unauthorized)?;
            let user = headers
                .get("x-test-user")
                .and_then(|value| value.to_str().ok())
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("user-{org}"));
            let mut response_headers = HeaderMap::new();
            response_headers.insert(header::SET_COOKIE, HeaderValue::from_static("seen=1"));
            Ok(Authenticated {
                identity: Identity {
                    user_id: user,
                    email: None,
                    org_id: org.to_string(),
                    org_name: None,
                },
                headers: response_headers,
            })
        })
    }
}

fn app(auth: Arc<dyn Authenticator>) -> Router {
    router(db::open_in_memory().unwrap(), auth, None)
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, headers, body)
}

fn get_request(path: &str, headers: &[(&str, &str)]) -> Request<Body> {
    let mut builder = Request::builder().uri(path);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(Body::empty()).unwrap()
}

fn post_json(path: &str, body: &Value, headers: &[(&str, &str)]) -> Request<Body> {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

fn post_transcript(session_id: &str, content: &str, headers: &[(&str, &str)]) -> Request<Body> {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri("/api/transcripts")
        .header(header::CONTENT_TYPE, "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder
        .body(Body::from(
            json!({ "session_id": session_id, "content": content }).to_string(),
        ))
        .unwrap()
}

#[tokio::test]
async fn healthz_reports_ok_without_auth() {
    let app = app(Arc::new(TokenAuth::new("secret")));
    let (status, _, body) = send(&app, get_request("/healthz", &[])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "status": "ok" }));
}

#[tokio::test]
async fn auth_config_reports_the_mode() {
    let (_, _, body) = send(&app(Arc::new(NoAuth)), get_request("/api/auth/config", &[])).await;
    assert_eq!(body, json!({ "mode": "none" }));
    let (status, _, body) = send(
        &app(Arc::new(TokenAuth::new("secret"))),
        get_request("/api/auth/config", &[]),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "mode": "token" }));
}

#[tokio::test]
async fn no_auth_uses_the_default_org() {
    let app = app(Arc::new(NoAuth));
    let (status, _, body) = send(&app, get_request("/api/me", &[])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({ "user_id": "default", "email": null, "org_id": "default", "org_name": null })
    );

    let (status, _, stored) = send(&app, post_transcript("s1", "hello", &[])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stored["session_id"], "s1");
    assert_eq!(stored["bytes"], 5);
    let (_, _, listed) = send(&app, get_request("/api/transcripts", &[])).await;
    assert_eq!(listed.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn token_auth_rejects_missing_and_wrong_tokens() {
    let app = app(Arc::new(TokenAuth::new("secret")));
    for headers in [
        vec![],
        vec![("authorization", "Bearer wrong")],
        vec![("authorization", "Basic secret")],
    ] {
        for request in [
            get_request("/api/transcripts", &headers),
            get_request("/api/me", &headers),
            post_transcript("s1", "x", &headers),
        ] {
            let (status, _, body) = send(&app, request).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(body, json!({ "error": "unauthorized" }));
        }
    }

    let auth = [("authorization", "Bearer secret")];
    let (status, _, _) = send(&app, post_transcript("s1", "x", &auth)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, listed) = send(&app, get_request("/api/transcripts", &auth)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed[0]["session_id"], "s1");
    let (_, _, me) = send(&app, get_request("/api/me", &auth)).await;
    assert_eq!(me["org_id"], "default");
}

#[tokio::test]
async fn orgs_cannot_see_each_others_transcripts() {
    let app = app(Arc::new(OrgHeaderAuth));
    let a = [("x-test-org", "org-a")];
    let b = [("x-test-org", "org-b")];
    send(&app, post_transcript("shared", "from a", &a)).await;
    send(&app, post_transcript("only-a", "a2", &a)).await;
    send(&app, post_transcript("shared", "from b", &b)).await;

    let (_, _, listed_a) = send(&app, get_request("/api/transcripts", &a)).await;
    let (_, _, listed_b) = send(&app, get_request("/api/transcripts", &b)).await;
    let ids = |listed: &Value| -> Vec<String> {
        let mut ids: Vec<String> = listed
            .as_array()
            .unwrap()
            .iter()
            .map(|t| format!("{}={}", t["session_id"], t["content"]))
            .collect();
        // Rows written in the same second share updated_at, so list order is not stable.
        ids.sort();
        ids
    };
    assert_eq!(
        ids(&listed_a),
        vec![r#""only-a"="a2""#, r#""shared"="from a""#]
    );
    assert_eq!(ids(&listed_b), vec![r#""shared"="from b""#]);
}

#[tokio::test]
async fn authenticator_headers_are_added_to_the_response() {
    let app = app(Arc::new(OrgHeaderAuth));
    let (status, headers, body) = send(&app, get_request("/api/me", &[("x-test-org", "o")])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["user_id"], "user-o");
    assert_eq!(headers.get(header::SET_COOKIE).unwrap(), "seen=1");
}

#[tokio::test]
async fn errors_are_json() {
    let app = app(Arc::new(NoAuth));
    let (status, _, body) = send(&app, get_request("/nope", &[])).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"].is_string());

    let (status, _, body) = send(&app, post_transcript("", "x", &[])).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, json!({ "error": "session_id is required" }));

    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/transcripts")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{not json"))
        .unwrap();
    let (status, _, body) = send(&app, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].is_string());
}

#[tokio::test]
async fn bodies_over_32_mib_are_rejected() {
    let app = app(Arc::new(NoAuth));
    let (status, _, _) = send(&app, post_transcript("big", &"x".repeat(31 << 20), &[])).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, body) = send(&app, post_transcript("big", &"x".repeat(33 << 20), &[])).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(body["error"].is_string());
}

#[tokio::test]
async fn extra_routes_are_merged_without_auth() {
    let extra = Router::new().route("/auth/ping", get(|| async { "pong" }));
    let app = router(
        db::open_in_memory().unwrap(),
        Arc::new(TokenAuth::new("secret")),
        Some(extra),
    );
    let response = app.oneshot(get_request("/auth/ping", &[])).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn memories_start_empty_and_stay_inside_the_org() {
    let app = app(Arc::new(OrgHeaderAuth));
    let a = [("x-test-org", "org-a"), ("x-test-user", "ada")];
    let b = [("x-test-org", "org-b"), ("x-test-user", "ada")];
    let (status, _, body) = send(&app, get_request("/api/memories", &a)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["files"].as_array().unwrap().len(), 0);
    assert_eq!(body["commits"].as_array().unwrap().len(), 0);

    let (status, _, saved) = send(
        &app,
        post_json(
            "/api/memories/commits",
            &json!({
                "message": "Add a note",
                "changes": [{ "op": "write", "path": "README.md", "content": "hello" }]
            }),
            &a,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["files"][0]["path"], "README.md");
    assert_eq!(saved["files"][0]["content"], "hello");
    assert_eq!(saved["files"][0]["blame"][0]["author"]["user_id"], "ada");
    assert_eq!(saved["commits"][0]["message"], "Add a note");
    assert_eq!(
        saved["files"][0]["blame"][0]["commit_id"],
        saved["commits"][0]["id"]
    );

    let (_, _, again) = send(&app, get_request("/api/memories", &a)).await;
    assert_eq!(again["commits"][0]["message"], "Add a note");
    let (_, _, other) = send(&app, get_request("/api/memories", &b)).await;
    assert_eq!(other["files"].as_array().unwrap().len(), 0);

    let (status, _, rejected) = send(
        &app,
        post_json(
            "/api/memories/commits",
            &json!({
                "message": "escape",
                "changes": [{ "op": "write", "path": "../secret", "content": "x" }]
            }),
            &a,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(rejected["error"], "invalid memory path");
    let (_, _, kept) = send(&app, get_request("/api/memories", &a)).await;
    assert_eq!(kept["files"][0]["content"], "hello");
}

#[tokio::test]
async fn personal_memories_are_a_separate_book() {
    let app = app(Arc::new(OrgHeaderAuth));
    let ada = [("x-test-org", "org-a"), ("x-test-user", "ada")];
    let sam = [("x-test-org", "org-a"), ("x-test-user", "sam")];
    let other = [("x-test-org", "org-b"), ("x-test-user", "ada")];

    let (status, _, body) = send(&app, get_request("/api/memories?scope=user", &ada)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["files"].as_array().unwrap().len(), 0);
    assert_eq!(body["commits"].as_array().unwrap().len(), 0);

    let note = json!({
        "message": "Upload notes.md",
        "changes": [{ "op": "write", "path": "notes.md", "content": "hello" }]
    });
    let (status, _, saved) = send(
        &app,
        post_json("/api/memories/commits?scope=user", &note, &ada),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["files"][0]["path"], "notes.md");

    let (_, _, team) = send(&app, get_request("/api/memories", &ada)).await;
    assert_eq!(team["files"].as_array().unwrap().len(), 0);
    let (_, _, team_named) = send(&app, get_request("/api/memories?scope=team", &ada)).await;
    assert_eq!(team_named["files"].as_array().unwrap().len(), 0);

    let (_, _, sam_book) = send(&app, get_request("/api/memories?scope=user", &sam)).await;
    assert_eq!(sam_book["files"].as_array().unwrap().len(), 0);
    let (_, _, ada_again) = send(&app, get_request("/api/memories?scope=user", &ada)).await;
    assert_eq!(ada_again["files"][0]["content"], "hello");
    let (_, _, other_book) = send(&app, get_request("/api/memories?scope=user", &other)).await;
    assert_eq!(other_book["files"].as_array().unwrap().len(), 0);

    let (status, _, emptied) = send(
        &app,
        post_json(
            "/api/memories/commits?scope=user",
            &json!({
                "message": "Remove the note",
                "changes": [{ "op": "delete", "path": "notes.md" }]
            }),
            &sam,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(emptied["error"], "invalid memory path");

    let (status, _, rejected) = send(&app, get_request("/api/memories?scope=nope", &ada)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(rejected["error"], "invalid memory scope");

    let (status, _, rejected) = send(
        &app,
        post_json(
            "/api/memories/commits?scope=user",
            &json!({
                "message": "escape",
                "changes": [{ "op": "write", "path": "../secret", "content": "x" }]
            }),
            &ada,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(rejected["error"], "invalid memory path");

    let (status, _, defaulted) = send(
        &app,
        post_json(
            "/api/memories/commits?scope=user",
            &json!({ "changes": [{ "op": "write", "path": "a.md", "content": "x" }] }),
            &ada,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(defaulted["files"][0]["content"], "x");
    assert_eq!(defaulted["commits"][0]["message"], "Update memory");
}

#[tokio::test]
async fn memory_directories_round_trip_and_cannot_match_a_file() {
    let app = app(Arc::new(OrgHeaderAuth));
    let ada = [("x-test-org", "org-a"), ("x-test-user", "ada")];
    let folder = json!({
        "message": "Create folder notes",
        "changes": [{ "op": "write", "path": "notes/.keep", "content": "" }]
    });
    let (status, _, saved) = send(
        &app,
        post_json("/api/memories/commits?scope=user", &folder, &ada),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["directories"], json!(["notes"]));
    assert_eq!(saved["files"].as_array().unwrap().len(), 0);

    let (_, _, again) = send(&app, get_request("/api/memories?scope=user", &ada)).await;
    assert_eq!(again["directories"], json!(["notes"]));

    let (status, _, rejected) = send(
        &app,
        post_json(
            "/api/memories/commits?scope=user",
            &json!({
                "message": "Clash",
                "changes": [{ "op": "write", "path": "notes", "content": "x" }]
            }),
            &ada,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(rejected["error"], "invalid memory path");

    let (_, _, kept) = send(&app, get_request("/api/memories?scope=user", &ada)).await;
    assert_eq!(kept["directories"], json!(["notes"]));
}

#[tokio::test]
async fn memories_require_the_api_token() {
    let app = app(Arc::new(TokenAuth::new("secret")));
    let (status, _, _) = send(&app, get_request("/api/memories", &[])).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, body) = send(
        &app,
        get_request("/api/memories", &[("authorization", "Bearer secret")]),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["files"].as_array().unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn git_push_keeps_client_shas_and_rejects_a_bad_tree() {
    let app = app(Arc::new(OrgHeaderAuth));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let served = app.clone();
    tokio::spawn(async move {
        axum::serve(listener, served).await.unwrap();
    });
    let root = std::env::temp_dir().join(format!("cadri-git-client-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let repo = root.join("client");
    let config = root.join("gitconfig");
    std::fs::write(
        &config,
        "[http]\n\textraHeader = x-test-org: org-a\n\textraHeader = x-test-user: ada\n[user]\n\tname = Ada\n\temail = ada@example.com\n",
    )
    .unwrap();
    let url = format!("http://{addr}/git/team.git");
    assert_ok(client(&repo, &config, &["init", "-b", "main"]));
    commit_file(
        &repo, &config, "note.md", "hello\n", "add note", "human", "ada",
    );
    let sha = rev_parse(&repo, &config, "HEAD");
    assert_ok(client(
        &repo,
        &config,
        &["push", &url, "HEAD:refs/heads/main"],
    ));
    let (_, _, body) = send(
        &app,
        get_request(
            "/api/memories",
            &[("x-test-org", "org-a"), ("x-test-user", "ada")],
        ),
    )
    .await;
    assert_eq!(body["commits"][0]["id"], sha);
    assert_eq!(body["files"][0]["content"], "hello\n");

    let other = root.join("other");
    std::fs::write(
        root.join("org-b"),
        "[http]\n\textraHeader = x-test-org: org-b\n\textraHeader = x-test-user: ada\n",
    )
    .unwrap();
    let listed = client(&root, root.join("org-b").as_path(), &["ls-remote", &url]);
    assert_ok(listed.clone());
    assert!(
        String::from_utf8_lossy(&listed.stdout).trim().is_empty(),
        "org b saw org a: {}",
        String::from_utf8_lossy(&listed.stderr)
    );

    let diverged = root.join("diverged");
    assert_ok(client(&diverged, &config, &["clone", &url, "."]));
    commit_file(
        &repo,
        &config,
        "note.md",
        "from ada\n",
        "ada edit",
        "human",
        "ada",
    );
    commit_file(
        &diverged,
        &config,
        "note.md",
        "from other\n",
        "other edit",
        "human",
        "ada",
    );
    assert_ok(client(
        &repo,
        &config,
        &["push", &url, "HEAD:refs/heads/main"],
    ));
    let rejected = client(&diverged, &config, &["push", &url, "HEAD:refs/heads/main"]);
    assert!(
        !rejected.status.success(),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    let remote = rev_parse(&repo, &config, "HEAD");
    let listed = client(&repo, &config, &["ls-remote", &url]);
    assert_ok(listed.clone());
    assert!(String::from_utf8_lossy(&listed.stdout).contains(&remote));

    std::fs::write(repo.join("bad.bin"), [0xff, 0xfe]).unwrap();
    assert_ok(client(&repo, &config, &["add", "bad.bin"]));
    commit_message(&repo, &config, "binary", "human", "ada");
    let binary = client(&repo, &config, &["push", &url, "HEAD:refs/heads/main"]);
    assert!(
        !binary.status.success(),
        "{}",
        String::from_utf8_lossy(&binary.stderr)
    );
    assert_ok(client(&repo, &config, &["reset", "--hard", &remote]));

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc/passwd", repo.join("link")).unwrap();
        assert_ok(client(&repo, &config, &["add", "link"]));
        commit_message(&repo, &config, "symlink", "human", "ada");
        let link = client(&repo, &config, &["push", &url, "HEAD:refs/heads/main"]);
        assert!(
            !link.status.success(),
            "{}",
            String::from_utf8_lossy(&link.stderr)
        );
        assert_ok(client(&repo, &config, &["reset", "--hard", &remote]));
    }

    commit_file(
        &repo,
        &config,
        "robot.md",
        "from an agent\n",
        "robot note",
        "robot",
        "agent-ada",
    );
    assert_ok(client(
        &repo,
        &config,
        &["push", &url, "HEAD:refs/heads/main"],
    ));
    let (_, _, store) = send(
        &app,
        get_request(
            "/api/memories",
            &[("x-test-org", "org-a"), ("x-test-user", "ada")],
        ),
    )
    .await;
    let robot = store["commits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|commit| commit["message"] == "robot note")
        .unwrap();
    assert_eq!(robot["author"]["kind"], "robot");
    assert_eq!(robot["author"]["id"], "agent-ada");
    let _ = (other, diverged);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn hosted_refuses_to_start_without_landlock() {
    if cadri_core::memory_jail_available() {
        return;
    }
    let output = Command::new(engine_binary())
        .env("CADRI_MEMORY_JAIL", "required")
        .env("CADRI_BIND", "127.0.0.1:9")
        .env("CADRI_DB", "/tmp/cadri-jail-should-not-open.db")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("landlock"), "{err}");
}

#[derive(Clone)]
struct GitOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn engine_binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.file_name().is_some_and(|name| name == "deps") {
        path.pop();
    }
    path.push("cadri-engine");
    path
}

fn client(dir: &Path, config: &Path, args: &[&str]) -> GitOutput {
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::create_dir_all(dir).ok();
    let output = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
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
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn rev_parse(dir: &Path, config: &Path, rev: &str) -> String {
    let output = client(dir, config, &["rev-parse", rev]);
    assert_ok(output.clone());
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn commit_file(
    dir: &Path,
    config: &Path,
    name: &str,
    content: &str,
    message: &str,
    kind: &str,
    id: &str,
) {
    std::fs::write(dir.join(name), content).unwrap();
    assert_ok(client(dir, config, &["add", name]));
    commit_message(dir, config, message, kind, id);
}

fn commit_message(dir: &Path, config: &Path, message: &str, kind: &str, id: &str) {
    let path = dir.join("COMMIT_EDITMSG_CADRI");
    std::fs::write(
        &path,
        format!("{message}\n\nCadri-Author-Kind: {kind}\nCadri-Author-Id: {id}\nCadri-Author-Label: {id}\n"),
    )
    .unwrap();
    assert_ok(client(
        dir,
        config,
        &["commit", "-F", "COMMIT_EDITMSG_CADRI"],
    ));
}

#[tokio::test]
async fn no_cors_headers_are_sent() {
    let app = app(Arc::new(NoAuth));
    let (_, headers, _) = send(
        &app,
        get_request("/api/transcripts", &[("origin", "https://evil.example")]),
    )
    .await;
    assert!(headers.get("access-control-allow-origin").is_none());
}
