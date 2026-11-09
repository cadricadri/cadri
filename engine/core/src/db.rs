use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::transcript;

/// Bump when the on-disk shape changes. Version 1 upgrades in place (see `upgrade_from_v1`);
/// any other version is dropped and recreated. Versions 2 and 3 were used by schemas from
/// before the reset to 1, so they still exist on disk and must not mean "current".
pub const SCHEMA_VERSION: i64 = 5;

/// Columns derived from `content` by `transcript::parse`, rewritten on every upload.
const SUMMARY_COLUMNS: &[(&str, &str)] = &[
    ("prompt", "TEXT"),
    ("started_at", "TEXT"),
    ("ended_at", "TEXT"),
    ("user_turns", "INTEGER NOT NULL DEFAULT 0"),
    ("assistant_turns", "INTEGER NOT NULL DEFAULT 0"),
    ("tool_calls", "INTEGER NOT NULL DEFAULT 0"),
    ("tool_counts", "TEXT"),
    ("has_writes", "INTEGER NOT NULL DEFAULT 0"),
];

const CREATE_SESSIONS: &str = "
CREATE TABLE sessions (
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
    prompt TEXT,
    started_at TEXT,
    ended_at TEXT,
    user_turns INTEGER NOT NULL DEFAULT 0,
    assistant_turns INTEGER NOT NULL DEFAULT 0,
    tool_calls INTEGER NOT NULL DEFAULT 0,
    tool_counts TEXT,
    has_writes INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (org_id, session_id)
);
CREATE INDEX sessions_org_updated ON sessions (org_id, updated_at DESC);
";

const PREVIEW_CHARS: usize = 240;

#[derive(Debug, Deserialize)]
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
    pub cwd: Option<String>,
    pub hook_event: Option<String>,
    /// Layer A blob. Present on POST response; omitted from list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
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
    cwd: Option<String>,
}

const PROMPT_CHARS: usize = 1000;

fn summarize(content: &str, conversation: Option<&Value>) -> Summary {
    let parsed = transcript::parse(content, conversation);
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
        cwd: parsed.cwd.clone(),
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
           assistant_turns = ?7, tool_calls = ?8, tool_counts = ?9, has_writes = ?10,
           model = COALESCE(model, ?11), cwd = COALESCE(cwd, ?12)
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
            summary.cwd,
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
    if version == 1 {
        upgrade_from_v1(&transaction)?;
    } else if version == 4 {
        rename_transcripts_table(&transaction)?;
    } else {
        // Unknown or pre-reset schemas are dropped. Versions 2 and 3 must not be trusted.
        transaction.execute_batch(
            "DROP TABLE IF EXISTS transcripts;
             DROP TABLE IF EXISTS sessions;
             DROP TABLE IF EXISTS memory_repos;",
        )?;
        transaction.execute_batch(CREATE_SESSIONS)?;
    }
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    transaction.commit()
}

fn upgrade_from_v1(connection: &Connection) -> rusqlite::Result<()> {
    for (name, kind) in SUMMARY_COLUMNS {
        connection.execute_batch(&format!("ALTER TABLE transcripts ADD COLUMN {name} {kind}"))?;
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
                    parse_conversation(row.get(3)?),
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    rename_transcripts_table(connection)?;
    for (org_id, session_id, content, conversation) in rows {
        let summary = summarize(&content, conversation.as_ref());
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

fn preview_for(incoming: &IncomingSession, content: &str) -> String {
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

fn row_to_session(row: &rusqlite::Row, include_layer_a: bool) -> rusqlite::Result<SessionRecord> {
    let session_id: String = row.get(0)?;
    let content: String = row.get(3)?;
    let conversation = parse_conversation(row.get(11)?);
    let transcript_text: Option<String> = row.get(12)?;
    let preview = match transcript_text.as_ref().filter(|value| !value.is_empty()) {
        Some(text) => preview(text),
        None => preview(&content),
    };
    let created_at: String = row.get(4)?;
    let updated_at: String = row.get(5)?;
    let client_timestamp: Option<String> = row.get(14)?;
    let parsed_start: Option<String> = row.get(16)?;
    let parsed_end: Option<String> = row.get(17)?;
    let ended_at = [
        parsed_end,
        client_timestamp.as_deref().and_then(normalize_time),
    ]
    .into_iter()
    .flatten()
    .max()
    .unwrap_or_else(|| updated_at.clone());
    let started_at = parsed_start
        .unwrap_or_else(|| created_at.clone())
        .min(ended_at.clone());
    let count = |index: usize| row.get::<_, i64>(index).map(|value| value.max(0) as u64);
    let user_turns = count(18)?;
    let assistant_turns = count(19)?;
    let tool_calls = count(20)?;
    let tool_counts = row
        .get::<_, Option<String>>(21)?
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    Ok(SessionRecord {
        thread_id: Some(session_id.clone()),
        session_id,
        cwd: row.get(1)?,
        hook_event: row.get(2)?,
        bytes: content.len(),
        preview,
        content: if include_layer_a { Some(content) } else { None },
        created_at,
        updated_at,
        user_id: row.get(6)?,
        harness: row.get(7)?,
        harness_version: row.get(8)?,
        model: row.get(9)?,
        model_version: row.get(10)?,
        conversation,
        transcript_text,
        compaction_summary: row.get(13)?,
        client_timestamp,
        prompt: row.get(15)?,
        started_at,
        ended_at,
        user_turns,
        assistant_turns,
        tool_call_count: tool_calls,
        event_count: user_turns + assistant_turns + tool_calls,
        tool_counts,
        has_writes: row.get(22)?,
    })
}

fn normalize_time(text: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(text).ok().map(|time| {
        time.with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true)
    })
}

const SELECT_COLS: &str = "session_id, cwd, hook_event, content, created_at, updated_at,
         user_id, harness, harness_version, model, model_version, conversation,
         transcript_text, compaction_summary, client_timestamp, prompt, started_at, ended_at,
         user_turns, assistant_turns, tool_calls, tool_counts, has_writes";

pub(crate) fn upsert(
    connection: &Connection,
    org_id: &str,
    user_id: &str,
    incoming: &IncomingSession,
) -> rusqlite::Result<SessionRecord> {
    let thread_id = incoming
        .thread_id()
        .expect("caller validates thread_id")
        .to_string();
    let content = incoming.layer_a_content();
    let timestamp = now();
    let conversation = conversation_json(&incoming.conversation);
    connection.execute(
        "INSERT INTO sessions (
            org_id, session_id, cwd, hook_event, content, created_at, updated_at,
            user_id, harness, harness_version, model, model_version, conversation,
            transcript_text, compaction_summary, client_timestamp
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
         ON CONFLICT(org_id, session_id) DO UPDATE SET
           cwd = COALESCE(excluded.cwd, sessions.cwd),
           hook_event = COALESCE(excluded.hook_event, sessions.hook_event),
           content = excluded.content,
           updated_at = excluded.updated_at,
           user_id = COALESCE(excluded.user_id, sessions.user_id),
           harness = COALESCE(excluded.harness, sessions.harness),
           harness_version = COALESCE(excluded.harness_version, sessions.harness_version),
           model = COALESCE(excluded.model, sessions.model),
           model_version = COALESCE(excluded.model_version, sessions.model_version),
           conversation = COALESCE(excluded.conversation, sessions.conversation),
           transcript_text = COALESCE(excluded.transcript_text, sessions.transcript_text),
           compaction_summary = COALESCE(excluded.compaction_summary, sessions.compaction_summary),
           client_timestamp = COALESCE(excluded.client_timestamp, sessions.client_timestamp)",
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
    let summary = summarize(&content, incoming.conversation.as_ref());
    write_summary(connection, org_id, &thread_id, &summary)?;

    connection.query_row(
        &format!("SELECT {SELECT_COLS} FROM sessions WHERE org_id = ?1 AND session_id = ?2"),
        params![org_id, thread_id],
        |row| {
            let mut session = row_to_session(row, true)?;
            let layer_a = session.content.as_deref().unwrap_or("");
            session.preview = preview_for(incoming, layer_a);
            Ok(session)
        },
    )
}

pub(crate) fn list(connection: &Connection, org_id: &str) -> rusqlite::Result<Vec<SessionRecord>> {
    let mut statement = connection.prepare(&format!(
        "SELECT {SELECT_COLS}
         FROM sessions
         WHERE org_id = ?1
         ORDER BY updated_at DESC"
    ))?;
    // List is Layer B + metadata: omit the Layer A `content` / trajectory blob.
    let rows = statement.query_map(params![org_id], |row| row_to_session(row, false))?;
    let mut rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    // A backfill uploads every session at once, so upload time says nothing about recency.
    rows.sort_by(|left, right| right.ended_at.cmp(&left.ended_at));
    Ok(rows)
}

/// One session with its Layer A blob, for the detail page.
pub(crate) fn get(
    connection: &Connection,
    org_id: &str,
    session_id: &str,
) -> rusqlite::Result<Option<SessionRecord>> {
    use rusqlite::OptionalExtension;
    connection
        .query_row(
            &format!("SELECT {SELECT_COLS} FROM sessions WHERE org_id = ?1 AND session_id = ?2"),
            params![org_id, session_id],
            |row| row_to_session(row, true),
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
            &IncomingSession {
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
            &IncomingSession {
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
        assert_eq!(second.content.as_deref(), Some("hello\nfull transcript"));
        assert_eq!(second.cwd.as_deref(), Some("/work"));
        assert_eq!(second.hook_event.as_deref(), Some("Stop"));
        assert_eq!(second.harness.as_deref(), Some("claude_code"));
        assert_eq!(second.user_id.as_deref(), Some("user-1"));
        assert_eq!(second.created_at, first.created_at);
        assert_eq!(
            second.bytes,
            second
                .content
                .as_ref()
                .map(|value| value.len())
                .unwrap_or(0)
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
        assert_eq!(stored.content.as_deref(), Some(r#"{"source":"cursor"}"#));
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
    fn list_omits_layer_a_content() {
        let connection = open_in_memory().unwrap();
        let stored = upsert(
            &connection,
            "default",
            "u",
            &IncomingSession {
                thread_id: None,
                session_id: Some("s1".to_string()),
                content: "huge trajectory blob".to_string(),
                trajectory: None,
                cwd: None,
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
        assert_eq!(stored.content.as_deref(), Some("huge trajectory blob"));

        let listed = list(&connection, "default").unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].content.is_none(), "list must omit Layer A");
        assert_eq!(listed[0].bytes, "huge trajectory blob".len());
        assert_eq!(
            listed[0].transcript_text.as_deref(),
            Some("user: hi\nassistant: hello")
        );
        assert!(listed[0].preview.starts_with("user: hi"));
        let json = serde_json::to_value(&listed[0]).unwrap();
        assert!(json.get("content").is_none());
    }

    #[test]
    fn same_session_id_is_independent_per_org() {
        let connection = open_in_memory().unwrap();
        upsert(&connection, "org-a", "u", &incoming("shared", "from a")).unwrap();
        upsert(&connection, "org-b", "u", &incoming("shared", "from b")).unwrap();
        assert_eq!(list(&connection, "org-a").unwrap()[0].preview, "from a");
        assert_eq!(list(&connection, "org-b").unwrap()[0].preview, "from b");
        assert!(list(&connection, "org-a").unwrap()[0].content.is_none());
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
        assert_eq!(stored.cwd.as_deref(), Some("/Users/ada/site"));
        assert_eq!(stored.started_at, "2026-10-07T18:43:41.432Z");
        assert_eq!(stored.ended_at, "2026-10-07T18:50:00.000Z");

        let fetched = get(&connection, "default", "claude").unwrap().unwrap();
        assert_eq!(fetched.content.as_deref(), Some(CLAUDE));
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
        let listed = list(&connection, "default").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].session_id, "kept");
        assert_eq!(listed[0].prompt.as_deref(), Some("hello"));
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
