//! HTTP router, auth gate, and request handlers.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, Path as UrlPath, Query, Request, State};
use axum::http::{header, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::{Authenticator, Identity};
use crate::db::{self, IncomingTranscript, Transcript};
use crate::memory::{self, Change, GitHttp, MemoryService, MemoryStore};
use crate::ApiError;

static MEMORY_ROOTS: AtomicU64 = AtomicU64::new(0);

const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone)]
struct AppState {
    db: Arc<Mutex<Connection>>,
    auth: Arc<dyn Authenticator>,
    memories: MemoryService,
}

/// Builds the engine router. `extra` is merged outside `require_auth`, e.g. ee's `/auth/*`
/// and billing routes, which authenticate themselves.
pub fn router(db: Connection, auth: Arc<dyn Authenticator>, extra: Option<Router>) -> Router {
    let n = MEMORY_ROOTS.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("cadri-memories-{}-{n}", std::process::id()));
    assemble(
        db,
        auth,
        extra,
        MemoryService::open_local(root, None, false),
    )
}

pub fn assemble(
    db: Connection,
    auth: Arc<dyn Authenticator>,
    extra: Option<Router>,
    memories: MemoryService,
) -> Router {
    let db = Arc::new(Mutex::new(db));
    auth.bind_db(Arc::clone(&db));
    let state = AppState { db, auth, memories };
    let protected = Router::new()
        .route("/api/me", get(me))
        .route(
            "/api/transcripts",
            get(list_transcripts).post(post_transcript),
        )
        .route("/api/memories", get(get_memories))
        .route("/api/memories/commits", post(post_memories))
        .route("/git/{book}/{*path}", get(git_http).post(git_http))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));
    let mut app = Router::new()
        .route("/healthz", get(healthz))
        .route("/api/auth/config", get(auth_config))
        .merge(protected)
        .with_state(state)
        .merge(crate::telemetry::routes());
    if let Some(extra) = extra {
        app = app.merge(extra);
    }
    // The metrics middleware is the outer layer so it times every route, including ee
    // routes merged above and the fallback.
    app.fallback(not_found)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(middleware::from_fn(crate::telemetry::track))
}

/// Opens `CADRI_DB`, listens on `CADRI_BIND` and serves until SIGINT/SIGTERM. Exits the
/// process on startup failure.
///
/// `CADRI_TLS_CERT` and `CADRI_TLS_KEY`, both set, serve HTTPS. Both unset serves HTTP.
pub async fn serve(auth: Arc<dyn Authenticator>, extra: Option<Router>) {
    if let Some(job) = memory::jail::job_from_args() {
        memory::jail::confine_and_exec(job);
    }
    let jail_required = std::env::var("CADRI_MEMORY_JAIL").ok().as_deref() == Some("required");
    if jail_required && !memory::jail_available() {
        eprintln!("CADRI_MEMORY_JAIL=required but landlock is unavailable; refusing to serve");
        std::process::exit(1);
    }
    let db_path = std::env::var("CADRI_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("data/transcripts.db"));
    let bind = std::env::var("CADRI_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
    let tls_cert = std::env::var("CADRI_TLS_CERT")
        .ok()
        .filter(|value| !value.is_empty());
    let tls_key = std::env::var("CADRI_TLS_KEY")
        .ok()
        .filter(|value| !value.is_empty());

    let connection = db::open(&db_path).unwrap_or_else(|err| {
        eprintln!("failed to open database {}: {err}", db_path.display());
        std::process::exit(1);
    });
    let memories_root = match std::env::var("CADRI_MEMORIES") {
        Ok(value) if !value.is_empty() => PathBuf::from(value),
        _ => db_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .join("memories"),
    };
    let mount = std::env::var("CADRI_MEMORIES_MOUNT")
        .ok()
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let memories = match std::env::var("CADRI_MEMORIES_BUCKET") {
        Ok(bucket) if !bucket.is_empty() => {
            let books = memory::S3Books::from_env(bucket)
                .await
                .unwrap_or_else(|err| {
                    eprintln!("failed to configure memory bucket: {err}");
                    std::process::exit(1);
                });
            MemoryService::open_s3(memories_root, mount, jail_required, books)
        }
        _ => MemoryService::open_local(memories_root, mount, jail_required),
    };
    let app = assemble(connection, auth.clone(), extra, memories);
    let scheme = if tls_cert.is_some() { "https" } else { "http" };
    eprintln!(
        "cadri engine listening on {scheme}://{bind} (database {}, auth {})",
        db_path.display(),
        auth.mode()
    );

    let result = match (tls_cert, tls_key) {
        (Some(cert), Some(key)) => serve_tls(&bind, &cert, &key, app).await,
        (None, None) => {
            let listener = tokio::net::TcpListener::bind(&bind)
                .await
                .unwrap_or_else(|err| {
                    eprintln!("failed to listen on {bind}: {err}");
                    std::process::exit(1);
                });
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown_signal())
                .await
        }
        _ => {
            eprintln!("CADRI_TLS_CERT and CADRI_TLS_KEY must both be set, or both be unset");
            std::process::exit(1);
        }
    };
    result.unwrap_or_else(|err| {
        eprintln!("server error: {err}");
        std::process::exit(1);
    });
}

async fn serve_tls(bind: &str, cert: &str, key: &str, app: Router) -> Result<(), std::io::Error> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let config =
        axum_server::tls_rustls::RustlsConfig::from_pem_file(Path::new(cert), Path::new(key))
            .await
            .unwrap_or_else(|err| {
                eprintln!("failed to load TLS certificate {cert}: {err}");
                std::process::exit(1);
            });
    let addr: SocketAddr = bind.parse().unwrap_or_else(|err| {
        eprintln!("CADRI_BIND {bind} is not a socket address: {err}");
        std::process::exit(1);
    });
    let handle = axum_server::Handle::new();
    let shutdown = handle.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        shutdown.graceful_shutdown(None);
    });
    axum_server::bind_rustls(addr, config)
        .handle(handle)
        .serve(app.into_make_service())
        .await
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

async fn require_auth(State(state): State<AppState>, mut request: Request, next: Next) -> Response {
    let path = request.uri().path().to_string();
    let authenticated = match state.auth.authorize(request.headers(), &path).await {
        Ok(authenticated) => authenticated,
        Err(err) => return err.into_response(),
    };
    request.extensions_mut().insert(authenticated.identity);
    let mut response = next.run(request).await;
    for (name, value) in &authenticated.headers {
        response.headers_mut().append(name, value.clone());
    }
    response
}

fn lock_db(state: &AppState) -> Result<MutexGuard<'_, Connection>, ApiError> {
    state
        .db
        .lock()
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "database lock poisoned"))
}

async fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not found")
}

async fn healthz(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let unavailable = || ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "database unavailable");
    let connection = state.db.lock().map_err(|_| unavailable())?;
    db::check(&connection).map_err(|err| {
        eprintln!("health check failed: {err}");
        unavailable()
    })?;
    Ok(Json(json!({ "status": "ok" })))
}

async fn auth_config(State(state): State<AppState>) -> Json<Value> {
    Json(json!({ "mode": state.auth.mode() }))
}

async fn me(Extension(identity): Extension<Identity>) -> Json<Identity> {
    Json(identity)
}

async fn post_transcript(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    body: Result<Json<IncomingTranscript>, JsonRejection>,
) -> Result<Json<Transcript>, ApiError> {
    let Json(incoming) = body?;
    if incoming.thread_id().is_none() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "session_id is required",
        ));
    }
    if !incoming.has_body() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "transcript body is required",
        ));
    }
    let connection = lock_db(&state)?;
    let stored =
        db::upsert(&connection, &identity.org_id, &identity.user_id, &incoming).map_err(|err| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to store transcript: {err}"),
            )
        })?;
    Ok(Json(stored))
}

#[derive(Debug, Deserialize)]
struct MemoryQuery {
    scope: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CommitRequest {
    #[serde(default)]
    message: String,
    #[serde(default)]
    changes: Vec<Change>,
}

fn memory_book(
    identity: &Identity,
    scope: Option<&str>,
) -> Result<(&'static str, String), ApiError> {
    match scope {
        None | Some("team") => Ok(("team", String::new())),
        Some("user") => Ok(("user", identity.user_id.clone())),
        Some(_) => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid memory scope",
        )),
    }
}

fn git_book(book: &str, identity: &Identity) -> Result<(&'static str, String), ApiError> {
    match book {
        "team.git" => Ok(("team", String::new())),
        "user.git" => Ok(("user", identity.user_id.clone())),
        _ => Err(ApiError::new(StatusCode::NOT_FOUND, "not found")),
    }
}

fn memory_repo(org_id: &str, scope: &str, user_id: &str) -> Result<String, ApiError> {
    memory::memory_repo_id(org_id, scope, user_id)
        .ok_or_else(|| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "invalid memory repo"))
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "memory task failed"))?
}

async fn get_memories(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    query: Result<Query<MemoryQuery>, QueryRejection>,
) -> Result<Json<MemoryStore>, ApiError> {
    let Query(query) = query?;
    let (scope, user_id) = memory_book(&identity, query.scope.as_deref())?;
    let repo_id = memory_repo(&identity.org_id, scope, &user_id)?;
    let service = state.memories.clone();
    let store = blocking(move || service.read_store(&repo_id)).await?;
    Ok(Json(store))
}

async fn post_memories(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    query: Result<Query<MemoryQuery>, QueryRejection>,
    body: Result<Json<CommitRequest>, JsonRejection>,
) -> Result<Json<MemoryStore>, ApiError> {
    let Query(query) = query?;
    let Json(request) = body?;
    let (scope, user_id) = memory_book(&identity, query.scope.as_deref())?;
    let repo_id = memory_repo(&identity.org_id, scope, &user_id)?;
    let service = state.memories.clone();
    let actor = identity.user_id.clone();
    let label = identity
        .email
        .clone()
        .unwrap_or_else(|| identity.user_id.clone());
    let store = blocking(move || {
        service.commit(&repo_id, &actor, &label, &request.message, &request.changes)
    })
    .await?;
    Ok(Json(store))
}

async fn git_http(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    UrlPath((book, path)): UrlPath<(String, String)>,
    request: Request,
) -> Result<Response, ApiError> {
    let (scope, user_id) = git_book(&book, &identity)?;
    let is_post = request.method() == Method::POST;
    let service_name = request.uri().query().and_then(query_param_service);
    let body = axum::body::to_bytes(request.into_body(), MAX_BODY_BYTES)
        .await
        .map_err(|_| ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "memory push is too large"))?;
    let repo_id = memory_repo(&identity.org_id, scope, &user_id)?;
    let actor = identity.user_id.clone();
    let service = state.memories.clone();
    let payload = body.to_vec();
    let reply = blocking(move || {
        service.serve_git(
            &repo_id,
            &path,
            is_post,
            service_name.as_deref(),
            &payload,
            &actor,
        )
    })
    .await?;
    let GitHttp::Ready { content_type, body } = reply;
    Ok(git_response(content_type, body))
}

fn query_param_service(query: &str) -> Option<String> {
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix("service=").map(|value| value.to_string()))
}

fn git_response(content_type: &'static str, body: Vec<u8>) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

async fn list_transcripts(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> Result<Json<Vec<Transcript>>, ApiError> {
    let connection = lock_db(&state)?;
    let transcripts = db::list(&connection, &identity.org_id).map_err(|err| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to list transcripts: {err}"),
        )
    })?;
    Ok(Json(transcripts))
}
