use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, Method, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tower_http::cors::{Any, CorsLayer};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS transcripts (
    session_id TEXT PRIMARY KEY,
    cwd TEXT,
    hook_event TEXT,
    content TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
";

const PREVIEW_CHARS: usize = 240;
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone)]
struct AppState {
    db: Arc<Mutex<Connection>>,
}

#[derive(Debug, Deserialize)]
struct IncomingTranscript {
    session_id: String,
    #[serde(default)]
    content: String,
    cwd: Option<String>,
    hook_event: Option<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct Transcript {
    session_id: String,
    cwd: Option<String>,
    hook_event: Option<String>,
    content: String,
    created_at: String,
    updated_at: String,
    bytes: usize,
    preview: String,
}

type ApiError = (StatusCode, String);

#[tokio::main]
async fn main() {
    let db_path = std::env::var("CADRI_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("data/transcripts.db"));
    let bind = std::env::var("CADRI_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_string());

    let connection = open_db(&db_path).unwrap_or_else(|err| {
        eprintln!("failed to open database {}: {err}", db_path.display());
        std::process::exit(1);
    });
    let state = AppState {
        db: Arc::new(Mutex::new(connection)),
    };

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .unwrap_or_else(|err| {
            eprintln!("failed to listen on {bind}: {err}");
            std::process::exit(1);
        });
    eprintln!(
        "cadri engine listening on http://{bind} (database {})",
        db_path.display()
    );

    axum::serve(listener, router(state))
        .await
        .unwrap_or_else(|err| {
            eprintln!("server error: {err}");
            std::process::exit(1);
        });
}

fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/transcripts",
            get(list_transcripts).post(post_transcript),
        )
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(cors())
        .with_state(state)
}

fn cors() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE])
}

fn open_db(path: &std::path::Path) -> Result<Connection, Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let connection = Connection::open(path)?;
    connection.execute_batch(SCHEMA)?;
    Ok(connection)
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn preview(content: &str) -> String {
    let mut chars = content.chars();
    let mut out: String = chars
        .by_ref()
        .take(PREVIEW_CHARS)
        .map(|ch| match ch {
            '\n' | '\r' => ' ',
            other => other,
        })
        .collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

fn row_to_transcript(
    row: (
        String,
        Option<String>,
        Option<String>,
        String,
        String,
        String,
    ),
) -> Transcript {
    let (session_id, cwd, hook_event, content, created_at, updated_at) = row;
    let bytes = content.len();
    let preview = preview(&content);
    Transcript {
        session_id,
        cwd,
        hook_event,
        content,
        created_at,
        updated_at,
        bytes,
        preview,
    }
}

fn upsert(
    connection: &Connection,
    incoming: &IncomingTranscript,
) -> Result<Transcript, rusqlite::Error> {
    let timestamp = now();
    connection.execute(
        "INSERT INTO transcripts (session_id, cwd, hook_event, content, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT(session_id) DO UPDATE SET
           cwd = COALESCE(excluded.cwd, transcripts.cwd),
           hook_event = COALESCE(excluded.hook_event, transcripts.hook_event),
           content = excluded.content,
           updated_at = excluded.updated_at",
        params![
            incoming.session_id,
            incoming.cwd,
            incoming.hook_event,
            incoming.content,
            timestamp,
        ],
    )?;

    connection
        .query_row(
            "SELECT session_id, cwd, hook_event, content, created_at, updated_at
             FROM transcripts WHERE session_id = ?1",
            params![incoming.session_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .map(row_to_transcript)
}

fn list(connection: &Connection) -> Result<Vec<Transcript>, rusqlite::Error> {
    let mut statement = connection.prepare(
        "SELECT session_id, cwd, hook_event, content, created_at, updated_at
         FROM transcripts
         ORDER BY updated_at DESC",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
        ))
    })?;
    rows.map(|row| row.map(row_to_transcript)).collect()
}

fn lock_db(state: &AppState) -> Result<std::sync::MutexGuard<'_, Connection>, ApiError> {
    state.db.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "database lock poisoned".to_string(),
        )
    })
}

async fn post_transcript(
    State(state): State<AppState>,
    Json(incoming): Json<IncomingTranscript>,
) -> Result<Json<Transcript>, ApiError> {
    if incoming.session_id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "session_id is required".to_string(),
        ));
    }
    let connection = lock_db(&state)?;
    let stored = upsert(&connection, &incoming).map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to store transcript: {err}"),
        )
    })?;
    Ok(Json(stored))
}

async fn list_transcripts(
    State(state): State<AppState>,
) -> Result<Json<Vec<Transcript>>, ApiError> {
    let connection = lock_db(&state)?;
    let transcripts = list(&connection).map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to list transcripts: {err}"),
        )
    })?;
    Ok(Json(transcripts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_db() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA).unwrap();
        connection
    }

    #[test]
    fn upsert_replaces_content_and_keeps_the_original_timestamp() {
        let connection = memory_db();
        let first = upsert(
            &connection,
            &IncomingTranscript {
                session_id: "session-1".to_string(),
                content: "hello".to_string(),
                cwd: Some("/work".to_string()),
                hook_event: Some("SessionStart".to_string()),
            },
        )
        .unwrap();

        let second = upsert(
            &connection,
            &IncomingTranscript {
                session_id: "session-1".to_string(),
                content: "hello\nfull transcript".to_string(),
                cwd: None,
                hook_event: Some("Stop".to_string()),
            },
        )
        .unwrap();

        assert_eq!(second.session_id, "session-1");
        assert_eq!(second.content, "hello\nfull transcript");
        assert_eq!(second.cwd.as_deref(), Some("/work"));
        assert_eq!(second.hook_event.as_deref(), Some("Stop"));
        assert_eq!(second.created_at, first.created_at);
        assert_eq!(second.bytes, second.content.len());

        let listed = list(&connection).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0], second);
    }

    #[test]
    fn list_returns_newest_update_first() {
        let connection = memory_db();
        upsert(
            &connection,
            &IncomingTranscript {
                session_id: "older".to_string(),
                content: "first".to_string(),
                cwd: None,
                hook_event: None,
            },
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        upsert(
            &connection,
            &IncomingTranscript {
                session_id: "newer".to_string(),
                content: "second".to_string(),
                cwd: None,
                hook_event: None,
            },
        )
        .unwrap();

        let listed = list(&connection).unwrap();
        assert_eq!(listed[0].session_id, "newer");
        assert_eq!(listed[1].session_id, "older");
    }

    #[test]
    fn preview_collapses_newlines_and_marks_truncation() {
        let content = format!("line\n{}", "x".repeat(300));
        let shown = preview(&content);
        assert!(shown.starts_with("line "));
        assert!(shown.ends_with('…'));
        assert!(shown.chars().count() <= PREVIEW_CHARS + 1);
    }
}
