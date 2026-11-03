use std::path::Path;
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Bump when the on-disk shape changes. Pre-production: migrate by dropping and recreating.
pub const SCHEMA_VERSION: i64 = 1;

const CREATE_TRANSCRIPTS: &str = "
CREATE TABLE transcripts (
    org_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    cwd TEXT,
    hook_event TEXT,
    content TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    user_id TEXT,
    harness TEXT,
    harness_version TEXT,
    model TEXT,
    model_version TEXT,
    conversation TEXT,
    transcript_text TEXT,
    compaction_summary TEXT,
    client_timestamp TEXT,
    PRIMARY KEY (org_id, session_id)
);
CREATE INDEX transcripts_org_updated ON transcripts (org_id, updated_at DESC);
";

const PREVIEW_CHARS: usize = 240;

#[derive(Debug, Deserialize)]
pub(crate) struct IncomingTranscript {
    /// Preferred client field; aliases `session_id`.
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub content: String,
    /// Layer A blob; if set and `content` is empty, stored as content.
    #[serde(default)]
    pub trajectory: Option<String>,
    pub cwd: Option<String>,
    pub hook_event: Option<String>,
    pub timestamp: Option<String>,
    pub conversation: Option<Value>,
    pub transcript_text: Option<String>,
    pub harness: Option<String>,
    pub harness_version: Option<String>,
    pub model: Option<String>,
    pub model_version: Option<String>,
    pub compaction_summary: Option<String>,
}

impl IncomingTranscript {
    pub(crate) fn thread_id(&self) -> Option<&str> {
        self.thread_id
            .as_deref()
            .filter(|value| !value.is_empty())
            .or_else(|| self.session_id.as_deref().filter(|value| !value.is_empty()))
    }

    pub(crate) fn layer_a_content(&self) -> String {
        if !self.content.is_empty() {
            return self.content.clone();
        }
        if let Some(trajectory) = self.trajectory.as_ref().filter(|value| !value.is_empty()) {
            return trajectory.clone();
        }
        if let Some(text) = self
            .transcript_text
            .as_ref()
            .filter(|value| !value.is_empty())
        {
            return text.clone();
        }
        if let Some(conversation) = &self.conversation {
            return conversation.to_string();
        }
        String::new()
    }

    pub(crate) fn has_body(&self) -> bool {
        !self.layer_a_content().is_empty()
            || self
                .transcript_text
                .as_ref()
                .is_some_and(|value| !value.is_empty())
            || self.conversation.is_some()
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct Transcript {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub cwd: Option<String>,
    pub hook_event: Option<String>,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
    pub bytes: usize,
    pub preview: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compaction_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_timestamp: Option<String>,
}

pub fn open(path: &Path) -> Result<Connection, Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let mut connection = Connection::open(path)?;
    prepare(&mut connection)?;
    Ok(connection)
}

pub fn open_in_memory() -> rusqlite::Result<Connection> {
    let mut connection = Connection::open_in_memory()?;
    prepare(&mut connection)?;
    Ok(connection)
}

// Hosted SQLite can sit on NFS. WAL mode needs shared memory that NFS does not
// provide reliably, so we force the classic DELETE rollback journal instead.
fn prepare(connection: &mut Connection) -> rusqlite::Result<()> {
    connection.busy_timeout(Duration::from_millis(5000))?;
    connection.pragma_update(None, "journal_mode", "DELETE")?;
    migrate(connection)
}

pub fn migrate(connection: &mut Connection) -> rusqlite::Result<()> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version == SCHEMA_VERSION {
        return Ok(());
    }
    let transaction = connection.transaction()?;
    // Pre-production: no transcript data worth preserving — drop and recreate.
    transaction.execute_batch(
        "DROP TABLE IF EXISTS transcripts;
         DROP TABLE IF EXISTS memory_repos;",
    )?;
    transaction.execute_batch(CREATE_TRANSCRIPTS)?;
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    transaction.commit()
}

pub(crate) fn check(connection: &Connection) -> rusqlite::Result<()> {
    connection.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
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

fn preview_for(incoming: &IncomingTranscript, content: &str) -> String {
    if let Some(text) = incoming
        .transcript_text
        .as_ref()
        .filter(|value| !value.is_empty())
    {
        return preview(text);
    }
    preview(content)
}

fn conversation_json(value: &Option<Value>) -> Option<String> {
    value.as_ref().map(|conversation| conversation.to_string())
}

fn parse_conversation(raw: Option<String>) -> Option<Value> {
    raw.and_then(|text| serde_json::from_str(&text).ok())
}

fn row_to_transcript(row: &rusqlite::Row) -> rusqlite::Result<Transcript> {
    let session_id: String = row.get(0)?;
    let content: String = row.get(3)?;
    let conversation = parse_conversation(row.get(11)?);
    Ok(Transcript {
        thread_id: Some(session_id.clone()),
        session_id,
        cwd: row.get(1)?,
        hook_event: row.get(2)?,
        bytes: content.len(),
        preview: preview(&content),
        content,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        user_id: row.get(6)?,
        harness: row.get(7)?,
        harness_version: row.get(8)?,
        model: row.get(9)?,
        model_version: row.get(10)?,
        conversation,
        transcript_text: row.get(12)?,
        compaction_summary: row.get(13)?,
        client_timestamp: row.get(14)?,
    })
}

const SELECT_COLS: &str = "session_id, cwd, hook_event, content, created_at, updated_at,
         user_id, harness, harness_version, model, model_version, conversation,
         transcript_text, compaction_summary, client_timestamp";

pub(crate) fn upsert(
    connection: &Connection,
    org_id: &str,
    user_id: &str,
    incoming: &IncomingTranscript,
) -> rusqlite::Result<Transcript> {
    let thread_id = incoming
        .thread_id()
        .expect("caller validates thread_id")
        .to_string();
    let content = incoming.layer_a_content();
    let timestamp = now();
    let conversation = conversation_json(&incoming.conversation);
    connection.execute(
        "INSERT INTO transcripts (
            org_id, session_id, cwd, hook_event, content, created_at, updated_at,
            user_id, harness, harness_version, model, model_version, conversation,
            transcript_text, compaction_summary, client_timestamp
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
         ON CONFLICT(org_id, session_id) DO UPDATE SET
           cwd = COALESCE(excluded.cwd, transcripts.cwd),
           hook_event = COALESCE(excluded.hook_event, transcripts.hook_event),
           content = excluded.content,
           updated_at = excluded.updated_at,
           user_id = COALESCE(excluded.user_id, transcripts.user_id),
           harness = COALESCE(excluded.harness, transcripts.harness),
           harness_version = COALESCE(excluded.harness_version, transcripts.harness_version),
           model = COALESCE(excluded.model, transcripts.model),
           model_version = COALESCE(excluded.model_version, transcripts.model_version),
           conversation = COALESCE(excluded.conversation, transcripts.conversation),
           transcript_text = COALESCE(excluded.transcript_text, transcripts.transcript_text),
           compaction_summary = COALESCE(excluded.compaction_summary, transcripts.compaction_summary),
           client_timestamp = COALESCE(excluded.client_timestamp, transcripts.client_timestamp)",
        params![
            org_id,
            thread_id,
            incoming.cwd,
            incoming.hook_event,
            content,
            timestamp,
            user_id,
            incoming.harness,
            incoming.harness_version,
            incoming.model,
            incoming.model_version,
            conversation,
            incoming.transcript_text,
            incoming.compaction_summary,
            incoming.timestamp,
        ],
    )?;

    connection.query_row(
        &format!("SELECT {SELECT_COLS} FROM transcripts WHERE org_id = ?1 AND session_id = ?2"),
        params![org_id, thread_id],
        |row| {
            let mut transcript = row_to_transcript(row)?;
            transcript.preview = preview_for(incoming, &transcript.content);
            Ok(transcript)
        },
    )
}

pub(crate) fn list(connection: &Connection, org_id: &str) -> rusqlite::Result<Vec<Transcript>> {
    let mut statement = connection.prepare(&format!(
        "SELECT {SELECT_COLS}
         FROM transcripts
         WHERE org_id = ?1
         ORDER BY updated_at DESC"
    ))?;
    let rows = statement.query_map(params![org_id], row_to_transcript)?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn incoming(session_id: &str, content: &str) -> IncomingTranscript {
        IncomingTranscript {
            thread_id: None,
            session_id: Some(session_id.to_string()),
            content: content.to_string(),
            trajectory: None,
            cwd: None,
            hook_event: None,
            timestamp: None,
            conversation: None,
            transcript_text: None,
            harness: None,
            harness_version: None,
            model: None,
            model_version: None,
            compaction_summary: None,
        }
    }

    fn user_version(connection: &Connection) -> i64 {
        connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn upsert_replaces_content_and_keeps_the_original_timestamp() {
        let connection = open_in_memory().unwrap();
        let first = upsert(
            &connection,
            "default",
            "user-1",
            &IncomingTranscript {
                thread_id: Some("session-1".to_string()),
                session_id: None,
                content: "hello".to_string(),
                trajectory: None,
                cwd: Some("/work".to_string()),
                hook_event: Some("SessionStart".to_string()),
                timestamp: None,
                conversation: None,
                transcript_text: None,
                harness: Some("claude_code".to_string()),
                harness_version: None,
                model: None,
                model_version: None,
                compaction_summary: None,
            },
        )
        .unwrap();

        let second = upsert(
            &connection,
            "default",
            "user-1",
            &IncomingTranscript {
                thread_id: None,
                session_id: Some("session-1".to_string()),
                content: "hello\nfull transcript".to_string(),
                trajectory: None,
                cwd: None,
                hook_event: Some("Stop".to_string()),
                timestamp: None,
                conversation: None,
                transcript_text: None,
                harness: Some("claude_code".to_string()),
                harness_version: None,
                model: None,
                model_version: None,
                compaction_summary: None,
            },
        )
        .unwrap();

        assert_eq!(second.session_id, "session-1");
        assert_eq!(second.thread_id.as_deref(), Some("session-1"));
        assert_eq!(second.content, "hello\nfull transcript");
        assert_eq!(second.cwd.as_deref(), Some("/work"));
        assert_eq!(second.hook_event.as_deref(), Some("Stop"));
        assert_eq!(second.harness.as_deref(), Some("claude_code"));
        assert_eq!(second.user_id.as_deref(), Some("user-1"));
        assert_eq!(second.created_at, first.created_at);
        assert_eq!(second.bytes, second.content.len());
    }

    #[test]
    fn thread_id_alias_and_trajectory_fill_content() {
        let connection = open_in_memory().unwrap();
        let stored = upsert(
            &connection,
            "default",
            "u",
            &IncomingTranscript {
                thread_id: Some("t1".to_string()),
                session_id: None,
                content: String::new(),
                trajectory: Some(r#"{"source":"cursor"}"#.to_string()),
                cwd: None,
                hook_event: Some("backfill".to_string()),
                timestamp: Some("2026-01-01T00:00:00.000Z".to_string()),
                conversation: Some(serde_json::json!([{"role":"user","text":"hi"}])),
                transcript_text: Some("user: hi".to_string()),
                harness: Some("cursor".to_string()),
                harness_version: Some("1".to_string()),
                model: Some("claude".to_string()),
                model_version: Some("4".to_string()),
                compaction_summary: Some("summary".to_string()),
            },
        )
        .unwrap();
        assert_eq!(stored.session_id, "t1");
        assert_eq!(stored.content, r#"{"source":"cursor"}"#);
        assert_eq!(stored.transcript_text.as_deref(), Some("user: hi"));
        assert_eq!(stored.model.as_deref(), Some("claude"));
        assert!(stored.conversation.is_some());
    }

    #[test]
    fn list_returns_newest_update_first() {
        let connection = open_in_memory().unwrap();
        upsert(&connection, "default", "u", &incoming("older", "first")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        upsert(&connection, "default", "u", &incoming("newer", "second")).unwrap();
        let listed = list(&connection, "default").unwrap();
        assert_eq!(listed[0].session_id, "newer");
        assert_eq!(listed[1].session_id, "older");
    }

    #[test]
    fn same_session_id_is_independent_per_org() {
        let connection = open_in_memory().unwrap();
        upsert(&connection, "org-a", "u", &incoming("shared", "from a")).unwrap();
        upsert(&connection, "org-b", "u", &incoming("shared", "from b")).unwrap();
        assert_eq!(list(&connection, "org-a").unwrap()[0].content, "from a");
        assert_eq!(list(&connection, "org-b").unwrap()[0].content, "from b");
    }

    #[test]
    fn migrate_nukes_an_old_schema() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE transcripts (
                    session_id TEXT PRIMARY KEY,
                    content TEXT NOT NULL
                );
                INSERT INTO transcripts VALUES ('old', 'gone');",
            )
            .unwrap();
        assert_eq!(user_version(&connection), 0);
        migrate(&mut connection).unwrap();
        assert_eq!(user_version(&connection), SCHEMA_VERSION);
        assert!(list(&connection, "default").unwrap().is_empty());
        upsert(&connection, "default", "u", &incoming("new", "ok")).unwrap();
        assert_eq!(list(&connection, "default").unwrap()[0].content, "ok");
    }

    #[test]
    fn migration_is_idempotent() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        upsert(&connection, "default", "u", &incoming("s", "kept")).unwrap();
        migrate(&mut connection).unwrap();
        assert_eq!(list(&connection, "default").unwrap()[0].content, "kept");
    }

    #[test]
    fn open_uses_delete_journal_mode() {
        let dir = std::env::temp_dir().join(format!("cadri-db-test-{}", std::process::id()));
        let path = dir.join("transcripts.db");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let connection = open(&path).unwrap();
        let journal_mode: String = connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode, "delete");
        drop(connection);
        std::fs::remove_dir_all(&dir).unwrap();
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
