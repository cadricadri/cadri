//! Postgres session store. Used when `CADRI_DATABASE_URL` is set.
//!
//! Schema changes are forward-only. A database written by a newer engine is
//! refused rather than migrated or emptied.

use std::time::Duration;

use sqlx::postgres::PgPoolOptions;
use sqlx::postgres::PgRow;
use sqlx::{Connection, PgPool, Row};

use crate::db::{self, IncomingSession, SessionFields, SessionRecord, SessionView, StoredSession};
use crate::schema::{self, Dialect};

pub(crate) const LATEST: i64 = 2;

const LOCK_KEY: i64 = 4_102_186;

pub(crate) fn reject_future_schema(current: i64) -> Result<(), String> {
    if current > LATEST {
        Err(format!(
            "database schema version {current} is newer than this engine ({LATEST}); refusing to start"
        ))
    } else {
        Ok(())
    }
}

pub(crate) async fn connect(url: &str) -> Result<PgPool, String> {
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(Duration::from_secs(2))
        .connect(url)
        .await
        .map_err(|err| format!("failed to connect to postgres: {err}"))?;
    migrate(&pool).await?;
    schema::check_postgres(&pool).await?;
    Ok(pool)
}

pub(crate) async fn migrate(pool: &PgPool) -> Result<(), String> {
    let mut connection = pool
        .acquire()
        .await
        .map_err(|err| format!("failed to acquire postgres connection: {err}"))?;
    crate::telemetry::time_postgres(
        "migrate",
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(LOCK_KEY)
            .execute(&mut *connection),
    )
    .await
    .map_err(|err| format!("failed to lock schema migration: {err}"))?;
    let result = migrate_locked(&mut connection).await;
    let unlock = crate::telemetry::time_postgres(
        "migrate",
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(LOCK_KEY)
            .execute(&mut *connection),
    )
    .await;
    result?;
    unlock.map_err(|err| format!("failed to unlock schema migration: {err}"))?;
    Ok(())
}

async fn migrate_locked(
    connection: &mut sqlx::pool::PoolConnection<sqlx::Postgres>,
) -> Result<(), String> {
    crate::telemetry::time_postgres(
        "migrate",
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL
            )",
        )
        .execute(&mut **connection),
    )
    .await
    .map_err(|err| format!("failed to create schema_migrations: {err}"))?;
    let current: i32 = crate::telemetry::time_postgres(
        "migrate",
        sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM schema_migrations")
            .fetch_one(&mut **connection),
    )
    .await
    .map_err(|err| format!("failed to read schema version: {err}"))?;
    reject_future_schema(i64::from(current))?;
    for version in (current + 1)..=i32::try_from(LATEST).expect("schema version fits in i32") {
        let mut transaction = crate::telemetry::time_postgres("migrate", connection.begin())
            .await
            .map_err(|err| format!("failed to start migration {version}: {err}"))?;
        for statement in migration_sql(i64::from(version)) {
            crate::telemetry::time_postgres(
                "migrate",
                sqlx::query(&statement).execute(&mut *transaction),
            )
            .await
            .map_err(|err| format!("migration {version} failed: {err}"))?;
        }
        crate::telemetry::time_postgres(
            "migrate",
            sqlx::query("INSERT INTO schema_migrations (version, applied_at) VALUES ($1, $2)")
                .bind(version)
                .bind(db::now())
                .execute(&mut *transaction),
        )
        .await
        .map_err(|err| format!("failed to record migration {version}: {err}"))?;
        transaction
            .commit()
            .await
            .map_err(|err| format!("failed to commit migration {version}: {err}"))?;
    }
    Ok(())
}

fn migration_sql(version: i64) -> Vec<String> {
    match version {
        1 => schema::create_statements(&schema::SESSIONS, Dialect::Postgres, false),
        2 => [
            &schema::BILLING,
            &schema::BILLING_EVENTS,
            &schema::SEEN_USERS,
        ]
        .into_iter()
        .flat_map(|table| schema::create_statements(table, Dialect::Postgres, false))
        .collect(),
        _ => Vec::new(),
    }
}

pub(crate) async fn check(pool: &PgPool) -> Result<(), String> {
    crate::telemetry::time_postgres("check", sqlx::query("SELECT 1").execute(pool))
        .await
        .map(|_| ())
        .map_err(|err| err.to_string())
}

pub(crate) async fn upsert(
    pool: &PgPool,
    org_id: &str,
    user_id: &str,
    incoming: &IncomingSession,
) -> Result<SessionRecord, String> {
    let write = db::session_write(incoming);
    let exists = schema::render(
        "SELECT 1 FROM sessions WHERE org_id = ?1 AND session_id = ?2",
        Dialect::Postgres,
    );
    let inserted = crate::telemetry::time_postgres(
        "session_read",
        sqlx::query_scalar::<_, i32>(&exists)
            .bind(org_id)
            .bind(&write.thread_id)
            .fetch_optional(pool),
    )
    .await
    .map_err(|err| err.to_string())?
    .is_none();
    let timestamp = db::now();
    let upsert_sql = schema::session_upsert(Dialect::Postgres);
    crate::telemetry::time_postgres(
        "session_upsert",
        sqlx::query(&upsert_sql)
            .bind(org_id)
            .bind(&write.thread_id)
            .bind(&incoming.hook_event)
            .bind(&timestamp)
            .bind(user_id)
            .bind(&incoming.harness)
            .bind(&incoming.harness_version)
            .bind(&incoming.model)
            .bind(&incoming.model_version)
            .bind(&incoming.timestamp)
            .bind(&write.preview)
            .bind(write.bytes)
            .bind(&write.view_json)
            .bind(&write.prompt)
            .bind(&write.started_at)
            .bind(&write.ended_at)
            .bind(write.user_turns)
            .bind(&write.summary_model)
            .bind(write.assistant_turns)
            .bind(write.tool_calls)
            .bind(&write.tool_counts)
            .bind(write.has_writes)
            .execute(pool),
    )
    .await
    .map_err(|err| format!("failed to store session: {err}"))?;
    if inserted {
        crate::telemetry::observe_session(write.bytes as u64);
    }
    load_record(pool, org_id, &write.thread_id)
        .await?
        .ok_or_else(|| "stored session was not readable".to_string())
}

pub(crate) async fn list(pool: &PgPool, org_id: &str) -> Result<Vec<SessionRecord>, String> {
    let sql = schema::session_list(Dialect::Postgres);
    let rows = crate::telemetry::time_postgres(
        "session_read",
        sqlx::query(&sql).bind(org_id).fetch_all(pool),
    )
    .await
    .map_err(|err| format!("failed to list sessions: {err}"))?;
    let mut sessions = rows
        .iter()
        .map(record_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    sessions.sort_by(|left, right| right.ended_at.cmp(&left.ended_at));
    Ok(sessions)
}

pub(crate) async fn get(
    pool: &PgPool,
    org_id: &str,
    session_id: &str,
) -> Result<Option<StoredSession>, String> {
    let sql = schema::session_with_view(Dialect::Postgres);
    let row = crate::telemetry::time_postgres(
        "session_read",
        sqlx::query(&sql)
            .bind(org_id)
            .bind(session_id)
            .fetch_optional(pool),
    )
    .await
    .map_err(|err| format!("failed to read session: {err}"))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let view = row
        .try_get::<String, _>("view")
        .ok()
        .and_then(|raw| serde_json::from_str::<SessionView>(&raw).ok())
        .unwrap_or_default();
    Ok(Some(StoredSession {
        session: record_from_row(&row)?,
        view,
    }))
}

pub(crate) async fn contributor_counts(pool: &PgPool) -> Result<Vec<i64>, String> {
    let sql = schema::contributor_counts(Dialect::Postgres);
    let rows = crate::telemetry::time_postgres(
        "contributors",
        sqlx::query_scalar::<_, i64>(&sql).fetch_all(pool),
    )
    .await
    .map_err(|err| err.to_string())?;
    Ok(rows)
}

async fn load_record(
    pool: &PgPool,
    org_id: &str,
    session_id: &str,
) -> Result<Option<SessionRecord>, String> {
    let sql = schema::session_by_id(Dialect::Postgres);
    let row = crate::telemetry::time_postgres(
        "session_read",
        sqlx::query(&sql)
            .bind(org_id)
            .bind(session_id)
            .fetch_optional(pool),
    )
    .await
    .map_err(|err| err.to_string())?;
    row.as_ref().map(record_from_row).transpose()
}

fn record_from_row(row: &PgRow) -> Result<SessionRecord, String> {
    let fields = SessionFields {
        session_id: row.try_get("session_id").map_err(|err| err.to_string())?,
        hook_event: row.try_get("hook_event").map_err(|err| err.to_string())?,
        created_at: row.try_get("created_at").map_err(|err| err.to_string())?,
        updated_at: row.try_get("updated_at").map_err(|err| err.to_string())?,
        user_id: row.try_get("user_id").map_err(|err| err.to_string())?,
        harness: row.try_get("harness").map_err(|err| err.to_string())?,
        harness_version: row
            .try_get("harness_version")
            .map_err(|err| err.to_string())?,
        model: row.try_get("model").map_err(|err| err.to_string())?,
        model_version: row
            .try_get("model_version")
            .map_err(|err| err.to_string())?,
        client_timestamp: row
            .try_get("client_timestamp")
            .map_err(|err| err.to_string())?,
        prompt: row.try_get("prompt").map_err(|err| err.to_string())?,
        preview: row.try_get("preview").map_err(|err| err.to_string())?,
        started_at: row.try_get("started_at").map_err(|err| err.to_string())?,
        ended_at: row.try_get("ended_at").map_err(|err| err.to_string())?,
        user_turns: row.try_get("user_turns").map_err(|err| err.to_string())?,
        assistant_turns: row
            .try_get("assistant_turns")
            .map_err(|err| err.to_string())?,
        tool_calls: row.try_get("tool_calls").map_err(|err| err.to_string())?,
        tool_counts: row.try_get("tool_counts").map_err(|err| err.to_string())?,
        has_writes: row.try_get("has_writes").map_err(|err| err.to_string())?,
        bytes: row.try_get("bytes").map_err(|err| err.to_string())?,
    };
    Ok(db::record_from_fields(fields))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newer_schema_version_is_refused() {
        assert!(reject_future_schema(LATEST).is_ok());
        assert!(reject_future_schema(LATEST + 1).is_err());
    }

    fn database_url() -> Option<String> {
        let url = std::env::var("CADRI_DATABASE_URL").ok()?;
        let url = url.trim();
        if url.is_empty() {
            None
        } else {
            Some(url.to_string())
        }
    }

    #[tokio::test]
    async fn upsert_keeps_harness_and_replaces_the_view() {
        let Some(url) = database_url() else {
            if std::env::var("CADRI_REQUIRE_POSTGRES").ok().as_deref() == Some("1") {
                panic!("CADRI_REQUIRE_POSTGRES=1 but CADRI_DATABASE_URL is empty");
            }
            return;
        };
        let pool = connect(&url).await.expect("connect");
        let org = format!("pg-{}", uuid_ish());
        let incoming = IncomingSession {
            thread_id: None,
            session_id: Some("s1".into()),
            content: r#"{"type":"user","message":{"role":"user","content":"fix the login"},"timestamp":"2026-10-07T18:00:00.000Z"}"#.into(),
            trajectory: None,
            hook_event: Some("stop".into()),
            timestamp: None,
            conversation: None,
            transcript_text: None,
            harness: Some("cursor".into()),
            harness_version: None,
            model: Some("composer".into()),
            model_version: None,
            compaction_summary: None,
        };
        let stored = upsert(&pool, &org, "user-1", &incoming)
            .await
            .expect("insert");
        assert_eq!(stored.harness.as_deref(), Some("cursor"));
        assert_eq!(stored.prompt.as_deref(), Some("fix the login"));

        let rewrite = IncomingSession {
            harness: None,
            model: None,
            content: r#"{"type":"user","message":{"role":"user","content":"fix the login"},"timestamp":"2026-10-07T18:00:00.000Z"}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Done."}]},"timestamp":"2026-10-07T18:01:00.000Z"}"#.into(),
            ..incoming
        };
        let updated = upsert(&pool, &org, "user-1", &rewrite)
            .await
            .expect("update");
        assert_eq!(updated.harness.as_deref(), Some("cursor"));
        assert_eq!(updated.model.as_deref(), Some("composer"));
        let fetched = get(&pool, &org, "s1").await.expect("get").expect("row");
        assert!(fetched.view.messages.len() >= 2);
        let listed = list(&pool, &org).await.expect("list");
        assert_eq!(listed.len(), 1);
        sqlx::query("DELETE FROM sessions WHERE org_id = $1")
            .bind(&org)
            .execute(&pool)
            .await
            .expect("cleanup");
    }

    fn uuid_ish() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        format!("{nanos}-{}", std::process::id())
    }
}
