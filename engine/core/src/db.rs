use std::path::Path;
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: i64 = 1;

const MIGRATION_1: &str = "
CREATE TABLE IF NOT EXISTS transcripts (
    session_id TEXT PRIMARY KEY,
    cwd TEXT,
    hook_event TEXT,
    content TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE transcripts_v1 (
    org_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    cwd TEXT,
    hook_event TEXT,
    content TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (org_id, session_id)
);
INSERT INTO transcripts_v1 (org_id, session_id, cwd, hook_event, content, created_at, updated_at)
    SELECT 'default', session_id, cwd, hook_event, content, created_at, updated_at FROM transcripts;
DROP TABLE transcripts;
ALTER TABLE transcripts_v1 RENAME TO transcripts;
CREATE INDEX transcripts_org_updated ON transcripts (org_id, updated_at DESC);
";

const PREVIEW_CHARS: usize = 240;

#[derive(Debug, Deserialize)]
pub(crate) struct IncomingTranscript {
    pub session_id: String,
    #[serde(default)]
    pub content: String,
    pub cwd: Option<String>,
    pub hook_event: Option<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct Transcript {
    session_id: String,
    cwd: Option<String>,
    hook_event: Option<String>,
    content: String,
    created_at: String,
    updated_at: String,
    bytes: usize,
    preview: String,
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

// The database lives on NFS in hosted deployments: WAL needs shared memory, so keep the
// rollback journal.
fn prepare(connection: &mut Connection) -> rusqlite::Result<()> {
    connection.busy_timeout(Duration::from_millis(5000))?;
    connection.pragma_update(None, "journal_mode", "DELETE")?;
    migrate(connection)
}

pub fn migrate(connection: &mut Connection) -> rusqlite::Result<()> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version >= SCHEMA_VERSION {
        return Ok(());
    }
    let transaction = connection.transaction()?;
    if version < 1 {
        transaction.execute_batch(MIGRATION_1)?;
    }
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

fn row_to_transcript(row: &rusqlite::Row) -> rusqlite::Result<Transcript> {
    let content: String = row.get(3)?;
    Ok(Transcript {
        session_id: row.get(0)?,
        cwd: row.get(1)?,
        hook_event: row.get(2)?,
        bytes: content.len(),
        preview: preview(&content),
        content,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

pub(crate) fn upsert(
    connection: &Connection,
    org_id: &str,
    incoming: &IncomingTranscript,
) -> rusqlite::Result<Transcript> {
    let timestamp = now();
    connection.execute(
        "INSERT INTO transcripts (org_id, session_id, cwd, hook_event, content, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
         ON CONFLICT(org_id, session_id) DO UPDATE SET
           cwd = COALESCE(excluded.cwd, transcripts.cwd),
           hook_event = COALESCE(excluded.hook_event, transcripts.hook_event),
           content = excluded.content,
           updated_at = excluded.updated_at",
        params![
            org_id,
            incoming.session_id,
            incoming.cwd,
            incoming.hook_event,
            incoming.content,
            timestamp,
        ],
    )?;

    connection.query_row(
        "SELECT session_id, cwd, hook_event, content, created_at, updated_at
         FROM transcripts WHERE org_id = ?1 AND session_id = ?2",
        params![org_id, incoming.session_id],
        row_to_transcript,
    )
}

pub(crate) fn list(connection: &Connection, org_id: &str) -> rusqlite::Result<Vec<Transcript>> {
    let mut statement = connection.prepare(
        "SELECT session_id, cwd, hook_event, content, created_at, updated_at
         FROM transcripts
         WHERE org_id = ?1
         ORDER BY updated_at DESC",
    )?;
    let rows = statement.query_map(params![org_id], row_to_transcript)?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD_SCHEMA: &str = "
    CREATE TABLE transcripts (
        session_id TEXT PRIMARY KEY,
        cwd TEXT,
        hook_event TEXT,
        content TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );
    ";

    fn incoming(session_id: &str, content: &str) -> IncomingTranscript {
        IncomingTranscript {
            session_id: session_id.to_string(),
            content: content.to_string(),
            cwd: None,
            hook_event: None,
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
            "default",
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

        let listed = list(&connection, "default").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0], second);
    }

    #[test]
    fn list_returns_newest_update_first() {
        let connection = open_in_memory().unwrap();
        upsert(&connection, "default", &incoming("older", "first")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        upsert(&connection, "default", &incoming("newer", "second")).unwrap();

        let listed = list(&connection, "default").unwrap();
        assert_eq!(listed[0].session_id, "newer");
        assert_eq!(listed[1].session_id, "older");
    }

    #[test]
    fn same_session_id_is_independent_per_org() {
        let connection = open_in_memory().unwrap();
        upsert(&connection, "org-a", &incoming("shared", "from a")).unwrap();
        upsert(&connection, "org-b", &incoming("shared", "from b")).unwrap();

        let a = list(&connection, "org-a").unwrap();
        let b = list(&connection, "org-b").unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].content, "from a");
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].content, "from b");
        assert!(list(&connection, "org-c").unwrap().is_empty());
    }

    #[test]
    fn migration_moves_existing_rows_into_the_default_org() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(OLD_SCHEMA).unwrap();
        connection
            .execute(
                "INSERT INTO transcripts VALUES ('old-session', '/work', 'Stop', 'old content',
                 '2026-01-01T00:00:00.000Z', '2026-01-02T00:00:00.000Z')",
                [],
            )
            .unwrap();
        assert_eq!(user_version(&connection), 0);

        migrate(&mut connection).unwrap();
        assert_eq!(user_version(&connection), SCHEMA_VERSION);

        let org_id: String = connection
            .query_row(
                "SELECT org_id FROM transcripts WHERE session_id = 'old-session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(org_id, "default");

        let listed = list(&connection, "default").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].session_id, "old-session");
        assert_eq!(listed[0].cwd.as_deref(), Some("/work"));
        assert_eq!(listed[0].hook_event.as_deref(), Some("Stop"));
        assert_eq!(listed[0].content, "old content");
        assert_eq!(listed[0].created_at, "2026-01-01T00:00:00.000Z");
        assert_eq!(listed[0].updated_at, "2026-01-02T00:00:00.000Z");

        upsert(&connection, "other", &incoming("old-session", "new org")).unwrap();
        assert_eq!(
            list(&connection, "default").unwrap()[0].content,
            "old content"
        );
        assert_eq!(list(&connection, "other").unwrap()[0].content, "new org");
    }

    #[test]
    fn migration_is_idempotent_and_creates_a_fresh_schema() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection).unwrap();
        upsert(&connection, "default", &incoming("s", "kept")).unwrap();
        migrate(&mut connection).unwrap();
        assert_eq!(user_version(&connection), SCHEMA_VERSION);
        assert_eq!(list(&connection, "default").unwrap()[0].content, "kept");
    }

    #[test]
    fn open_migrates_an_old_database_file_without_wal() {
        let dir = std::env::temp_dir().join(format!("cadri-db-test-{}", std::process::id()));
        let path = dir.join("transcripts.db");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        {
            let connection = Connection::open(&path).unwrap();
            connection.execute_batch(OLD_SCHEMA).unwrap();
            connection
                .execute(
                    "INSERT INTO transcripts VALUES ('s', NULL, NULL, 'x', 't0', 't1')",
                    [],
                )
                .unwrap();
        }

        let connection = open(&path).unwrap();
        let journal_mode: String = connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode, "delete");
        assert_eq!(user_version(&connection), SCHEMA_VERSION);
        assert_eq!(list(&connection, "default").unwrap()[0].session_id, "s");
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
