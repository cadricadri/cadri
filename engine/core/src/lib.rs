pub mod auth;
pub mod db;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use rusqlite::Connection;
use serde_json::{json, Value};

pub use auth::{AuthFuture, Authenticated, Authenticator, Identity};
use db::{IncomingTranscript, Transcript};

const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

/// Rendered as `{"error": message}` with `status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        ApiError {
            status,
            message: message.into(),
        }
    }

    pub fn unauthorized() -> Self {
        ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

impl From<JsonRejection> for ApiError {
    fn from(rejection: JsonRejection) -> Self {
        ApiError::new(rejection.status(), rejection.body_text())
    }
}

impl From<QueryRejection> for ApiError {
    fn from(rejection: QueryRejection) -> Self {
        ApiError::new(rejection.status(), rejection.body_text())
    }
}

#[derive(Clone)]
struct AppState {
    db: Arc<Mutex<Connection>>,
    auth: Arc<dyn Authenticator>,
}

/// Builds the engine router. `extra` is merged as-is (unauthenticated), e.g. ee's `/auth/*`.
pub fn router(db: Connection, auth: Arc<dyn Authenticator>, extra: Option<Router>) -> Router {
    let state = AppState {
        db: Arc::new(Mutex::new(db)),
        auth,
    };
    let protected = Router::new()
        .route("/api/me", get(me))
        .route(
            "/api/transcripts",
            get(list_transcripts).post(post_transcript),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));
    let mut app = Router::new()
        .route("/healthz", get(healthz))
        .route("/api/auth/config", get(auth_config))
        .merge(protected)
        .with_state(state);
    if let Some(extra) = extra {
        app = app.merge(extra);
    }
    app.fallback(not_found)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
}

/// Opens `CADRI_DB`, listens on `CADRI_BIND` and serves until SIGINT/SIGTERM. Exits the
/// process on startup failure.
///
/// `CADRI_TLS_CERT` and `CADRI_TLS_KEY`, both set, serve HTTPS. Both unset serves HTTP.
pub async fn serve(auth: Arc<dyn Authenticator>, extra: Option<Router>) {
    let db_path = std::env::var("CADRI_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("data/transcripts.db"));
    let bind = std::env::var("CADRI_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
    let tls_cert = std::env::var("CADRI_TLS_CERT").ok().filter(|value| !value.is_empty());
    let tls_key = std::env::var("CADRI_TLS_KEY").ok().filter(|value| !value.is_empty());

    let connection = db::open(&db_path).unwrap_or_else(|err| {
        eprintln!("failed to open database {}: {err}", db_path.display());
        std::process::exit(1);
    });
    let app = router(connection, auth.clone(), extra);
    let scheme = if tls_cert.is_some() { "https" } else { "http" };
    eprintln!(
        "cadri engine listening on {scheme}://{bind} (database {}, auth {})",
        db_path.display(),
        auth.mode()
    );

    let result = match (tls_cert, tls_key) {
        (Some(cert), Some(key)) => serve_tls(&bind, &cert, &key, app).await,
        (None, None) => {
            let listener = tokio::net::TcpListener::bind(&bind).await.unwrap_or_else(|err| {
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

async fn serve_tls(
    bind: &str,
    cert: &str,
    key: &str,
    app: Router,
) -> Result<(), std::io::Error> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(Path::new(cert), Path::new(key))
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
    let authenticated = match state.auth.authenticate(request.headers()).await {
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
    if incoming.session_id.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "session_id is required",
        ));
    }
    let connection = lock_db(&state)?;
    let stored = db::upsert(&connection, &identity.org_id, &incoming).map_err(|err| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to store transcript: {err}"),
        )
    })?;
    Ok(Json(stored))
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
