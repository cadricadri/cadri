use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::schema;
use crate::transcript;

/// Bump when the on-disk shape changes. Version 1 adds summary columns and renames
/// `transcripts` to `sessions`. Version 4 renames that table. Version 5 is the sessions
/// table with a `cwd` column; version 6 drops it. Version 7 keeps the parsed chat,
/// plans, and tool names, and drops the raw transcript. Versions 2 and 3 predate the
/// reset to 1 and are dropped.
pub const SCHEMA_VERSION: i64 = 7;

/// Columns derived from `content` by `transcript::parse`, added when upgrading a v1 file.
const SUMMARY_COLUMNS: &[&str] = &[
    "prompt",
    "started_at",
    "ended_at",
    "user_turns",
    "assistant_turns",
    "tool_calls",
    "tool_counts",
    "has_writes",
];

const PREVIEW_CHARS: usize = 240;

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct IncomingSession {
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
    pub hook_event: Option<String>,
    pub timestamp: Option<String>,
    pub conversation: Option<Value>,
    pub transcript_text: Option<String>,
    pub harness: Option<String>,
    pub harness_version: Option<String>,
    pub model: Option<String>,
    pub model_version: Option<String>,
    /// Accepted from older clients. Not stored.
    #[allow(dead_code)]
    pub compaction_summary: Option<String>,
}

impl IncomingSession {
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
pub(crate) struct SessionRecord {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub hook_event: Option<String>,
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
    pub client_timestamp: Option<String>,
    /// First thing the person typed, from the parsed session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Session start from the uploaded session itself, else when the server first saw it.
    pub started_at: String,
    /// Session end from the uploaded session itself or the uploaded file time, else last upload.
    pub ended_at: String,
    pub user_turns: u64,
    pub assistant_turns: u64,
    pub tool_call_count: u64,
    pub event_count: u64,
    pub tool_counts: BTreeMap<String, u64>,
    pub has_writes: bool,
}

/// What `transcript::parse` found in one stored blob.
struct Summary {
    prompt: Option<String>,
    started_at: Option<String>,
    ended_at: Option<String>,
    user_turns: i64,
    assistant_turns: i64,
    tool_calls: i64,
    tool_counts: String,
    has_writes: bool,
    model: Option<String>,
}

const PROMPT_CHARS: usize = 1000;

/// What the session page renders. The raw harness transcript is parsed on ingest
/// and not stored; it remains on the machine that uploaded it.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub(crate) struct SessionView {
    #[serde(default)]
    pub messages: Vec<transcript::Message>,
    #[serde(default)]
    pub tool_calls: Vec<transcript::ToolCall>,
    #[serde(default)]
    pub plans: Vec<transcript::Plan>,
    #[serde(default)]
    pub canvases: Vec<transcript::Canvas>,
}

impl SessionView {
    fn from_parsed(parsed: transcript::Parsed) -> Self {
        Self {
            messages: parsed.messages,
            tool_calls: parsed.tool_calls,
            plans: parsed.plans,
            canvases: parsed.canvases,
        }
    }
}

pub(crate) struct StoredSession {
    pub session: SessionRecord,
    pub view: SessionView,
}

fn summary_from(parsed: &transcript::Parsed) -> Summary {
    let prompt = parsed
        .prompt()
        .map(|text| text.chars().take(PROMPT_CHARS).collect());
    Summary {
        prompt,
        started_at: parsed.started_at(),
        ended_at: parsed.ended_at(),
        user_turns: parsed.turns(transcript::Role::User) as i64,
        assistant_turns: parsed.turns(transcript::Role::Assistant) as i64,
        tool_calls: parsed.tool_calls.len() as i64,
        tool_counts: serde_json::to_string(&parsed.tool_counts()).unwrap_or_else(|_| "{}".into()),
        has_writes: parsed.has_writes(),
        model: parsed.model.clone(),
    }
}

fn write_summary(
    connection: &Connection,
    org_id: &str,
    session_id: &str,
    summary: &Summary,
) -> rusqlite::Result<()> {
    connection.execute(
        "UPDATE sessions SET
           prompt = ?3, started_at = ?4, ended_at = ?5, user_turns = ?6,
           assistant_turns = ?7, tool_calls = ?8, tool_counts = ?9,            has_writes = ?10,
           model = COALESCE(model, ?11)
         WHERE org_id = ?1 AND session_id = ?2",
        params![
            org_id,
            session_id,
            summary.prompt,
            summary.started_at,
            summary.ended_at,
            summary.user_turns,
            summary.assistant_turns,
            summary.tool_calls,
            summary.tool_counts,
            summary.has_writes,
            summary.model,
        ],
    )?;
    Ok(())
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

// Localhost and the self-host binary use this file. DELETE journaling keeps the
// database in one file, without WAL shared-memory sidecars.
fn prepare(connection: &mut Connection) -> rusqlite::Result<()> {
    connection.busy_timeout(Duration::from_millis(5000))?;
    connection.pragma_update(None, "journal_mode", "DELETE")?;
    migrate(connection)
}

fn drop_cwd_column(connection: &Connection) -> rusqlite::Result<()> {
    let names = {
        let mut statement = connection.prepare("PRAGMA table_info(sessions)")?;
        let names = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        names
    };
    if names.iter().any(|name| name == "cwd") {
        connection.execute_batch("ALTER TABLE sessions DROP COLUMN cwd")?;
    }
    Ok(())
}

pub fn migrate(connection: &mut Connection) -> rusqlite::Result<()> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version == SCHEMA_VERSION {
        return schema::check_sqlite(connection, &schema::SESSIONS)
            .map_err(rusqlite::Error::InvalidColumnName);
    }
    let project = {
        let transaction = connection.transaction()?;
        // Versions 2 and 3 predate the reset to 1 and are dropped. Anything else
        // unknown is rebuilt empty. Versions 1, 4, 5, and 6 keep their rows.
        let project = match version {
            1 => {
                upgrade_from_v1(&transaction)?;
                true
            }
            4 => {
                rename_transcripts_table(&transaction)?;
                true
            }
            5 | 6 => true,
            _ => {
                transaction.execute_batch(
                    "DROP TABLE IF EXISTS transcripts;
                     DROP TABLE IF EXISTS sessions;
                     DROP TABLE IF EXISTS memory_repos;",
                )?;
                transaction.execute_batch(&schema::sqlite_script(&schema::SESSIONS, false))?;
                false
            }
        };
        drop_cwd_column(&transaction)?;
        if project {
            project_sessions(&transaction)?;
        }
        for statement in schema::index_statements(&schema::SESSIONS, true) {
            transaction.execute_batch(&format!("{statement};"))?;
        }
        schema::check_sqlite(&transaction, &schema::SESSIONS)
            .map_err(rusqlite::Error::InvalidColumnName)?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        transaction.commit()?;
        project
    };
    // Rewriting every transcript leaves the old pages allocated until vacuum.
    if project {
        connection.execute_batch("VACUUM")?;
    }
    Ok(())
}

/// Parse each stored transcript and keep the chat, plans, and tool names.
fn project_sessions(connection: &Connection) -> rusqlite::Result<()> {
    if !has_column(connection, "sessions", "content") {
        return Ok(());
    }
    ensure_column(connection, "sessions", "preview")?;
    ensure_column(connection, "sessions", "bytes")?;
    ensure_column(connection, "sessions", "view")?;
    let rows = {
        let mut statement = connection.prepare(
            "SELECT org_id, session_id, content, conversation, transcript_text FROM sessions",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for (org_id, session_id, content, conversation, transcript_text) in rows {
        let conversation = conversation
            .as_deref()
            .and_then(|raw| serde_json::from_str(raw).ok());
        let parsed = transcript::parse(&content, conversation.as_ref());
        let shown = preview_text(transcript_text.as_deref(), &parsed, &content);
        let view = SessionView::from_parsed(parsed);
        let view_json = serde_json::to_string(&view).unwrap_or_else(|_| "{}".into());
        connection.execute(
            "UPDATE sessions SET preview = ?3, bytes = ?4, view = ?5
             WHERE org_id = ?1 AND session_id = ?2",
            params![org_id, session_id, shown, view_json.len() as i64, view_json],
        )?;
    }
    for column in [
        "content",
        "conversation",
        "transcript_text",
        "compaction_summary",
    ] {
        drop_column(connection, "sessions", column)?;
    }
    Ok(())
}

fn ensure_column(connection: &Connection, table: &str, column: &str) -> rusqlite::Result<()> {
    if has_column(connection, table, column) {
        return Ok(());
    }
    let definition = schema::sqlite_column_clause(&schema::SESSIONS, column);
    connection.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {definition}"))
}

fn drop_column(connection: &Connection, table: &str, column: &str) -> rusqlite::Result<()> {
    if has_column(connection, table, column) {
        connection.execute_batch(&format!("ALTER TABLE {table} DROP COLUMN {column}"))?;
    }
    Ok(())
}

fn has_column(connection: &Connection, table: &str, column: &str) -> bool {
    let Ok(mut statement) = connection.prepare(&format!("PRAGMA table_info({table})")) else {
        return false;
    };
    statement
        .query_map([], |row| row.get::<_, String>(1))
        .ok()
        .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>().ok())
        .is_some_and(|names| names.iter().any(|name| name == column))
}

fn upgrade_from_v1(connection: &Connection) -> rusqlite::Result<()> {
    for name in SUMMARY_COLUMNS {
        let definition = schema::sqlite_column_clause(&schema::SESSIONS, name);
        connection.execute_batch(&format!("ALTER TABLE transcripts ADD COLUMN {definition}"))?;
    }
    let rows = {
        let mut statement = connection
            .prepare("SELECT org_id, session_id, content, conversation FROM transcripts")?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?
                        .and_then(|raw| serde_json::from_str(&raw).ok()),
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    rename_transcripts_table(connection)?;
    for (org_id, session_id, content, conversation) in rows {
        let parsed = transcript::parse(&content, conversation.as_ref());
        let summary = summary_from(&parsed);
        write_summary(connection, &org_id, &session_id, &summary)?;
    }
    Ok(())
}

/// Version 4 stored the same rows in `transcripts`. Keep them under the sessions name.
fn rename_transcripts_table(connection: &Connection) -> rusqlite::Result<()> {
    let has_table: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'transcripts'",
        [],
        |row| row.get(0),
    )?;
    if has_table == 0 {
        return Ok(());
    }
    connection.execute_batch(
        "ALTER TABLE transcripts RENAME TO sessions;
         DROP INDEX IF EXISTS transcripts_org_updated;
         CREATE INDEX IF NOT EXISTS sessions_org_updated ON sessions (org_id, updated_at DESC);",
    )?;
    Ok(())
}

pub(crate) fn check(connection: &Connection) -> rusqlite::Result<()> {
    connection.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
}

pub(crate) fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Parsed session ready to store. The raw transcript is not part of this.
pub(crate) struct SessionWrite {
    pub thread_id: String,
    pub preview: String,
    pub view_json: String,
    pub bytes: i64,
    pub prompt: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub user_turns: i64,
    pub assistant_turns: i64,
    pub tool_calls: i64,
    pub tool_counts: String,
    pub has_writes: bool,
    pub summary_model: Option<String>,
}

pub(crate) fn session_write(incoming: &IncomingSession) -> SessionWrite {
    let thread_id = incoming
        .thread_id()
        .expect("caller validates thread_id")
        .to_string();
    let raw = incoming.layer_a_content();
    let parsed = transcript::parse(&raw, incoming.conversation.as_ref());
    let summary = summary_from(&parsed);
    let shown = preview_text(incoming.transcript_text.as_deref(), &parsed, &raw);
    let view = SessionView::from_parsed(parsed);
    let view_json = serde_json::to_string(&view).unwrap_or_else(|_| "{}".into());
    SessionWrite {
        thread_id,
        preview: shown,
        bytes: view_json.len() as i64,
        view_json,
        prompt: summary.prompt,
        started_at: summary.started_at,
        ended_at: summary.ended_at,
        user_turns: summary.user_turns,
        assistant_turns: summary.assistant_turns,
        tool_calls: summary.tool_calls,
        tool_counts: summary.tool_counts,
        has_writes: summary.has_writes,
        summary_model: summary.model,
    }
}

pub(crate) struct SessionFields {
    pub session_id: String,
    pub hook_event: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub user_id: Option<String>,
    pub harness: Option<String>,
    pub harness_version: Option<String>,
    pub model: Option<String>,
    pub model_version: Option<String>,
    pub client_timestamp: Option<String>,
    pub prompt: Option<String>,
    pub preview: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub user_turns: i64,
    pub assistant_turns: i64,
    pub tool_calls: i64,
    pub tool_counts: Option<String>,
    pub has_writes: bool,
    pub bytes: i64,
}

pub(crate) fn record_from_fields(fields: SessionFields) -> SessionRecord {
    let ended_at = [
        fields.ended_at,
        fields.client_timestamp.as_deref().and_then(normalize_time),
    ]
    .into_iter()
    .flatten()
    .max()
    .unwrap_or_else(|| fields.updated_at.clone());
    let started_at = fields
        .started_at
        .unwrap_or_else(|| fields.created_at.clone())
        .min(ended_at.clone());
    let user_turns = fields.user_turns.max(0) as u64;
    let assistant_turns = fields.assistant_turns.max(0) as u64;
    let tool_calls = fields.tool_calls.max(0) as u64;
    let tool_counts = fields
        .tool_counts
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    SessionRecord {
        thread_id: Some(fields.session_id.clone()),
        session_id: fields.session_id,
        hook_event: fields.hook_event,
        bytes: fields.bytes.max(0) as usize,
        preview: fields.preview,
        created_at: fields.created_at,
        updated_at: fields.updated_at,
        user_id: fields.user_id,
        harness: fields.harness,
        harness_version: fields.harness_version,
        model: fields.model,
        model_version: fields.model_version,
        client_timestamp: fields.client_timestamp,
        prompt: fields.prompt,
        started_at,
        ended_at,
        user_turns,
        assistant_turns,
        tool_call_count: tool_calls,
        event_count: user_turns + assistant_turns + tool_calls,
        tool_counts,
        has_writes: fields.has_writes,
    }
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

fn preview_text(transcript_text: Option<&str>, parsed: &transcript::Parsed, raw: &str) -> String {
    if let Some(text) = transcript_text.filter(|value| !value.is_empty()) {
        return preview(text);
    }
    if let Some(text) = parsed.prompt().filter(|text| !text.is_empty()) {
        return preview(text);
    }
    preview(raw)
}

fn row_to_session(row: &rusqlite::Row) -> rusqlite::Result<SessionRecord> {
    Ok(record_from_fields(SessionFields {
        session_id: row.get("session_id")?,
        hook_event: row.get("hook_event")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        user_id: row.get("user_id")?,
        harness: row.get("harness")?,
        harness_version: row.get("harness_version")?,
        model: row.get("model")?,
        model_version: row.get("model_version")?,
        client_timestamp: row.get("client_timestamp")?,
        prompt: row.get("prompt")?,
        preview: row.get("preview")?,
        started_at: row.get("started_at")?,
        ended_at: row.get("ended_at")?,
        user_turns: row.get("user_turns")?,
        assistant_turns: row.get("assistant_turns")?,
        tool_calls: row.get("tool_calls")?,
        tool_counts: row.get("tool_counts")?,
        has_writes: row.get("has_writes")?,
        bytes: row.get("bytes")?,
    }))
}

fn normalize_time(text: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(text).ok().map(|time| {
        time.with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true)
    })
}

pub(crate) fn upsert(
    connection: &Connection,
    org_id: &str,
    user_id: &str,
    incoming: &IncomingSession,
) -> rusqlite::Result<SessionRecord> {
    let write = session_write(incoming);
    let inserted = connection
        .query_row(
            "SELECT 1 FROM sessions WHERE org_id = ?1 AND session_id = ?2",
            params![org_id, &write.thread_id],
            |_| Ok(1_i64),
        )
        .optional()?
        .is_none();
    let timestamp = now();
    let sql = schema::session_upsert(schema::Dialect::Sqlite);
    let values: [&dyn rusqlite::types::ToSql; 22] = [
        &org_id,
        &write.thread_id,
        &incoming.hook_event,
        &timestamp,
        &user_id,
        &incoming.harness,
        &incoming.harness_version,
        &incoming.model,
        &incoming.model_version,
        &incoming.timestamp,
        &write.preview,
        &write.bytes,
        &write.view_json,
        &write.prompt,
        &write.started_at,
        &write.ended_at,
        &write.user_turns,
        &write.summary_model,
        &write.assistant_turns,
        &write.tool_calls,
        &write.tool_counts,
        &write.has_writes,
    ];
    connection.execute(&sql, &values[..])?;

    let session = connection.query_row(
        &schema::session_by_id(schema::Dialect::Sqlite),
        params![org_id, write.thread_id],
        row_to_session,
    )?;
    if inserted {
        crate::telemetry::observe_session(write.bytes as u64);
    }
    Ok(session)
}

pub(crate) fn list(connection: &Connection, org_id: &str) -> rusqlite::Result<Vec<SessionRecord>> {
    let mut statement = connection.prepare(&schema::session_list(schema::Dialect::Sqlite))?;
    let rows = statement.query_map(params![org_id], row_to_session)?;
    let mut rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    // A backfill uploads every session at once, so upload time says nothing about recency.
    rows.sort_by(|left, right| right.ended_at.cmp(&left.ended_at));
    Ok(rows)
}

/// One session and the chat the detail page renders.
pub(crate) fn get(
    connection: &Connection,
    org_id: &str,
    session_id: &str,
) -> rusqlite::Result<Option<StoredSession>> {
    connection
        .query_row(
            &schema::session_with_view(schema::Dialect::Sqlite),
            params![org_id, session_id],
            |row| {
                let view = row
                    .get::<_, String>("view")
                    .ok()
                    .and_then(|raw| serde_json::from_str(&raw).ok())
                    .unwrap_or_default();
                Ok(StoredSession {
                    session: row_to_session(row)?,
                    view,
                })
            },
        )
        .optional()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn incoming(session_id: &str, content: &str) -> IncomingSession {
        IncomingSession {
            thread_id: None,
            session_id: Some(session_id.to_string()),
            content: content.to_string(),
            trajectory: None,
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

    fn has_column(connection: &Connection, table: &str, column: &str) -> bool {
        let mut statement = connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap();
        let names: Vec<String> = statement
            .query_map([], |row| row.get(1))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        names.iter().any(|name| name == column)
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
            &IncomingSession {
                thread_id: Some("session-1".to_string()),
                session_id: None,
                content: "hello".to_string(),
                trajectory: None,
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
            &IncomingSession {
                thread_id: None,
                session_id: Some("session-1".to_string()),
                content: "hello\nfull transcript".to_string(),
                trajectory: None,
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
        assert_eq!(second.preview, "hello full transcript");
        assert!(!has_column(&connection, "sessions", "content"));
        assert_eq!(second.hook_event.as_deref(), Some("Stop"));
        assert_eq!(second.harness.as_deref(), Some("claude_code"));
        assert_eq!(second.user_id.as_deref(), Some("user-1"));
        assert_eq!(second.created_at, first.created_at);
    }

    fn count_sessions(connection: &Connection, org_id: &str) -> i64 {
        connection
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE org_id = ?1",
                params![org_id],
                |row| row.get(0),
            )
            .unwrap()
    }

    /// A turn uploads the whole transcript again under the same id. That rewrites one row.
    #[test]
    fn mid_turn_uploads_rewrite_one_session_row() {
        let connection = open_in_memory().unwrap();
        let init = r#"{"type":"user","message":{"role":"user","content":"fix the login"},"timestamp":"2026-10-07T18:00:00.000Z"}"#;
        let mid = format!(
            "{init}\n{}",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Reading auth.rs."}]},"timestamp":"2026-10-07T18:01:00.000Z"}"#
        );
        let later = format!(
            "{mid}\n{}",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"Read"},{"type":"text","text":"Fixed the session check."}]},"timestamp":"2026-10-07T18:02:00.000Z"}"#
        );

        let first = upsert(
            &connection,
            "default",
            "user-1",
            &incoming("sess-live", init),
        )
        .unwrap();
        assert_eq!(count_sessions(&connection, "default"), 1);
        assert_eq!(first.user_turns, 1);
        assert_eq!(first.assistant_turns, 0);
        assert_eq!(first.tool_call_count, 0);

        std::thread::sleep(Duration::from_millis(5));
        let second = upsert(
            &connection,
            "default",
            "user-1",
            &incoming("sess-live", &mid),
        )
        .unwrap();
        assert_eq!(count_sessions(&connection, "default"), 1);
        assert_eq!(second.created_at, first.created_at);
        assert!(second.updated_at > first.updated_at);
        assert!(second.bytes < mid.len());
        assert_eq!(second.assistant_turns, 1);
        assert_eq!(second.tool_call_count, 0);
        assert_eq!(list(&connection, "default").unwrap().len(), 1);

        let mut continued = incoming("sess-live", &later);
        continued.thread_id = Some("sess-live".to_string());
        continued.session_id = None;
        continued.hook_event = Some("Stop".to_string());
        let third = upsert(&connection, "default", "user-1", &continued).unwrap();
        assert_eq!(count_sessions(&connection, "default"), 1);
        assert_eq!(third.session_id, "sess-live");
        assert_eq!(third.created_at, first.created_at);
        assert!(third.bytes < later.len());
        assert_eq!(third.prompt.as_deref(), Some("fix the login"));
        assert_eq!(third.user_turns, 1);
        assert_eq!(third.assistant_turns, 1);
        assert_eq!(third.tool_call_count, 1);
        assert_eq!(third.hook_event.as_deref(), Some("Stop"));

        let listed = list(&connection, "default").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].session_id, "sess-live");
        assert!(listed[0].bytes < later.len());
        assert_eq!(listed[0].tool_call_count, 1);
        let stored = get(&connection, "default", "sess-live").unwrap().unwrap();
        assert_eq!(stored.view.messages.len(), 2);
        assert_eq!(stored.view.messages[0].text, "fix the login");
        assert_eq!(stored.view.tool_calls.len(), 1);

        upsert(
            &connection,
            "default",
            "user-1",
            &incoming("sess-other", "a different session"),
        )
        .unwrap();
        assert_eq!(count_sessions(&connection, "default"), 2);
        let mut ids: Vec<_> = list(&connection, "default")
            .unwrap()
            .into_iter()
            .map(|session| session.session_id)
            .collect();
        ids.sort();
        assert_eq!(ids, vec!["sess-live".to_string(), "sess-other".to_string()]);
        assert_eq!(
            get(&connection, "default", "sess-live")
                .unwrap()
                .unwrap()
                .view
                .messages[0]
                .text,
            "fix the login"
        );
    }

    #[test]
    fn thread_id_alias_and_trajectory_fill_content() {
        let connection = open_in_memory().unwrap();
        let stored = upsert(
            &connection,
            "default",
            "u",
            &IncomingSession {
                thread_id: Some("t1".to_string()),
                session_id: None,
                content: String::new(),
                trajectory: Some(r#"{"source":"cursor"}"#.to_string()),
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
        assert_eq!(stored.preview, "user: hi");
        assert_eq!(stored.prompt.as_deref(), Some("hi"));
        assert_eq!(stored.model.as_deref(), Some("claude"));
        assert!(!has_column(&connection, "sessions", "content"));
        assert!(!has_column(&connection, "sessions", "transcript_text"));
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
    fn list_omits_layer_a_content() {
        let connection = open_in_memory().unwrap();
        let stored = upsert(
            &connection,
            "default",
            "u",
            &IncomingSession {
                thread_id: None,
                session_id: Some("s1".to_string()),
                content: "x".repeat(10_000),
                trajectory: None,
                hook_event: None,
                timestamp: None,
                conversation: None,
                transcript_text: Some("user: hi\nassistant: hello".to_string()),
                harness: Some("cursor".to_string()),
                harness_version: None,
                model: None,
                model_version: None,
                compaction_summary: None,
            },
        )
        .unwrap();
        assert!(stored.bytes < 10_000);
        assert_eq!(stored.preview, "user: hi assistant: hello");

        let listed = list(&connection, "default").unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].bytes < 10_000);
        assert!(listed[0].preview.starts_with("user: hi"));
        let json = serde_json::to_value(&listed[0]).unwrap();
        assert!(json.get("content").is_none());
        assert!(json.get("transcript_text").is_none());
    }

    #[test]
    fn same_session_id_is_independent_per_org() {
        let connection = open_in_memory().unwrap();
        upsert(&connection, "org-a", "u", &incoming("shared", "from a")).unwrap();
        upsert(&connection, "org-b", "u", &incoming("shared", "from b")).unwrap();
        assert_eq!(list(&connection, "org-a").unwrap()[0].preview, "from a");
        assert_eq!(list(&connection, "org-b").unwrap()[0].preview, "from b");
        assert!(!has_column(&connection, "sessions", "content"));
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
        assert_eq!(list(&connection, "default").unwrap()[0].preview, "ok");
    }

    const CLAUDE: &str = include_str!("transcript/fixtures/claude_code.jsonl");

    #[test]
    fn upsert_stores_the_parsed_summary() {
        let connection = open_in_memory().unwrap();
        let mut upload = incoming("claude", CLAUDE);
        upload.timestamp = Some("2026-10-07T18:50:00Z".to_string());
        let stored = upsert(&connection, "default", "u", &upload).unwrap();
        assert_eq!(stored.prompt.as_deref(), Some("install the vercel cli"));
        assert_eq!(stored.user_turns, 1);
        assert_eq!(stored.assistant_turns, 1);
        assert_eq!(stored.tool_call_count, 2);
        assert_eq!(stored.event_count, 4);
        assert!(stored.has_writes);
        assert_eq!(stored.tool_counts.get("Bash"), Some(&1));
        assert_eq!(stored.model.as_deref(), Some("claude-opus-4"));
        assert_eq!(stored.started_at, "2026-10-07T18:43:41.432Z");
        assert_eq!(stored.ended_at, "2026-10-07T18:50:00.000Z");

        let fetched = get(&connection, "default", "claude").unwrap().unwrap();
        assert!(!fetched.view.messages.is_empty());
        assert!(fetched.session.bytes < CLAUDE.len());
        assert!(!has_column(&connection, "sessions", "content"));
        assert!(get(&connection, "default", "missing").unwrap().is_none());
    }

    #[test]
    fn unparsed_content_falls_back_to_server_times() {
        let connection = open_in_memory().unwrap();
        let stored = upsert(&connection, "default", "u", &incoming("plain", "hello")).unwrap();
        assert_eq!(stored.started_at, stored.created_at);
        assert_eq!(stored.ended_at, stored.updated_at);
        assert_eq!(stored.event_count, 0);
        assert!(stored.prompt.is_none());
    }

    #[test]
    fn upgrade_from_v1_keeps_rows_and_fills_summaries() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE transcripts (
                    org_id TEXT NOT NULL, session_id TEXT NOT NULL, cwd TEXT, hook_event TEXT,
                    content TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
                    user_id TEXT, harness TEXT, harness_version TEXT, model TEXT,
                    model_version TEXT, conversation TEXT, transcript_text TEXT,
                    compaction_summary TEXT, client_timestamp TEXT,
                    PRIMARY KEY (org_id, session_id)
                );
                PRAGMA user_version = 1;",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO transcripts (org_id, session_id, content, created_at, updated_at)
                 VALUES ('default', 'kept', ?1, '2026-10-08T00:00:00.000Z', '2026-10-08T00:00:00.000Z')",
                params![CLAUDE],
            )
            .unwrap();
        migrate(&mut connection).unwrap();
        assert_eq!(user_version(&connection), SCHEMA_VERSION);
        assert!(!has_column(&connection, "sessions", "cwd"));
        let listed = list(&connection, "default").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].prompt.as_deref(), Some("install the vercel cli"));
        assert_eq!(listed[0].model.as_deref(), Some("claude-opus-4"));
        assert_eq!(listed[0].started_at, "2026-10-07T18:43:41.432Z");
    }

    #[test]
    fn upgrade_from_v4_renames_the_table_and_keeps_rows() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE transcripts (
                    org_id TEXT NOT NULL, session_id TEXT NOT NULL, cwd TEXT, hook_event TEXT,
                    content TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
                    user_id TEXT, harness TEXT, harness_version TEXT, model TEXT,
                    model_version TEXT, conversation TEXT, transcript_text TEXT,
                    compaction_summary TEXT, client_timestamp TEXT,
                    prompt TEXT, started_at TEXT, ended_at TEXT,
                    user_turns INTEGER NOT NULL DEFAULT 0,
                    assistant_turns INTEGER NOT NULL DEFAULT 0,
                    tool_calls INTEGER NOT NULL DEFAULT 0,
                    tool_counts TEXT, has_writes INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (org_id, session_id)
                );
                CREATE INDEX transcripts_org_updated ON transcripts (org_id, updated_at DESC);
                PRAGMA user_version = 4;",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO transcripts (
                    org_id, session_id, content, created_at, updated_at, prompt, started_at, ended_at
                 ) VALUES (
                    'default', 'kept', 'hello', '2026-10-08T00:00:00.000Z', '2026-10-08T00:00:00.000Z',
                    'hello', '2026-10-08T00:00:00.000Z', '2026-10-08T00:00:00.000Z'
                 )",
                [],
            )
            .unwrap();
        migrate(&mut connection).unwrap();
        assert_eq!(user_version(&connection), SCHEMA_VERSION);
        assert!(!has_column(&connection, "sessions", "cwd"));
        let listed = list(&connection, "default").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].session_id, "kept");
        assert_eq!(listed[0].prompt.as_deref(), Some("hello"));
    }

    #[test]
    fn upgrade_from_v5_drops_cwd_and_keeps_rows() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE sessions (
                    org_id TEXT NOT NULL, session_id TEXT NOT NULL, cwd TEXT, hook_event TEXT,
                    content TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
                    user_id TEXT, harness TEXT, harness_version TEXT, model TEXT,
                    model_version TEXT, conversation TEXT, transcript_text TEXT,
                    compaction_summary TEXT, client_timestamp TEXT,
                    prompt TEXT, started_at TEXT, ended_at TEXT,
                    user_turns INTEGER NOT NULL DEFAULT 0,
                    assistant_turns INTEGER NOT NULL DEFAULT 0,
                    tool_calls INTEGER NOT NULL DEFAULT 0,
                    tool_counts TEXT, has_writes INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (org_id, session_id)
                );
                PRAGMA user_version = 5;",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO sessions (
                    org_id, session_id, cwd, content, created_at, updated_at, prompt, started_at, ended_at
                 ) VALUES (
                    'default', 'kept', '/secret/folder', 'hello',
                    '2026-10-08T00:00:00.000Z', '2026-10-08T00:00:00.000Z',
                    'hello', '2026-10-08T00:00:00.000Z', '2026-10-08T00:00:00.000Z'
                 )",
                [],
            )
            .unwrap();
        migrate(&mut connection).unwrap();
        assert_eq!(user_version(&connection), SCHEMA_VERSION);
        assert!(!has_column(&connection, "sessions", "cwd"));
        let listed = list(&connection, "default").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].session_id, "kept");
        assert_eq!(listed[0].prompt.as_deref(), Some("hello"));
    }

    #[test]
    fn upgrade_from_v6_keeps_the_chat_and_drops_the_transcript() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE sessions (
                    org_id TEXT NOT NULL, session_id TEXT NOT NULL, hook_event TEXT,
                    content TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
                    user_id TEXT, harness TEXT, harness_version TEXT, model TEXT,
                    model_version TEXT, conversation TEXT, transcript_text TEXT,
                    compaction_summary TEXT, client_timestamp TEXT,
                    prompt TEXT, started_at TEXT, ended_at TEXT,
                    user_turns INTEGER NOT NULL DEFAULT 0,
                    assistant_turns INTEGER NOT NULL DEFAULT 0,
                    tool_calls INTEGER NOT NULL DEFAULT 0,
                    tool_counts TEXT, has_writes INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (org_id, session_id)
                );
                PRAGMA user_version = 6;",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO sessions (
                    org_id, session_id, content, created_at, updated_at, prompt
                 ) VALUES (
                    'default', 'kept', ?1, '2026-10-08T00:00:00.000Z', '2026-10-08T00:00:00.000Z',
                    'install the vercel cli'
                 )",
                params![CLAUDE],
            )
            .unwrap();
        migrate(&mut connection).unwrap();
        assert!(!has_column(&connection, "sessions", "content"));
        assert!(!has_column(&connection, "sessions", "transcript_text"));
        let stored = get(&connection, "default", "kept").unwrap().unwrap();
        assert!(!stored.view.messages.is_empty());
        assert!(stored.session.bytes < CLAUDE.len());
        assert_eq!(
            stored.session.prompt.as_deref(),
            Some("install the vercel cli")
        );
    }

    #[test]
    fn a_stale_version_2_schema_is_rebuilt_not_trusted() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE transcripts (
                    org_id TEXT NOT NULL, session_id TEXT NOT NULL, cwd TEXT, hook_event TEXT,
                    content TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
                    PRIMARY KEY (org_id, session_id)
                );
                PRAGMA user_version = 2;",
            )
            .unwrap();
        migrate(&mut connection).unwrap();
        assert_eq!(user_version(&connection), SCHEMA_VERSION);
        upsert(&connection, "default", "u", &incoming("new", "ok")).unwrap();
        assert_eq!(list(&connection, "default").unwrap()[0].preview, "ok");
    }

    #[test]
    fn migration_is_idempotent() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        upsert(&connection, "default", "u", &incoming("s", "kept")).unwrap();
        migrate(&mut connection).unwrap();
        assert_eq!(list(&connection, "default").unwrap()[0].preview, "kept");
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
