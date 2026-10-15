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
            let mut response_headers = HeaderMap::new();
            response_headers.insert(header::SET_COOKIE, HeaderValue::from_static("seen=1"));
            Ok(Authenticated {
                identity: Identity {
                    user_id: format!("user-{org}"),
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
        listed
            .as_array()
            .unwrap()
            .iter()
            .map(|t| format!("{}={}", t["session_id"], t["content"]))
            .collect()
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
async fn no_cors_headers_are_sent() {
    let app = app(Arc::new(NoAuth));
    let (_, headers, _) = send(
        &app,
        get_request("/api/transcripts", &[("origin", "https://evil.example")]),
    )
    .await;
    assert!(headers.get("access-control-allow-origin").is_none());
}
