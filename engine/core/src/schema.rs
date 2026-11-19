//! Tables and statements shared by SQLite and Postgres.
//!
//! A column is declared once. `create_statements` renders it for one dialect
//! (SQLite `INTEGER`, Postgres `BIGINT`, booleans as integers or `BOOLEAN`).
//! Statements are written with `?1` placeholders; `render` turns those into
//! `$1` for Postgres. After migrate, `check_sqlite` / `check_postgres` read the
//! live catalog and refuse a database whose columns, types, nulls, defaults,
//! primary key, or indexes disagree.
//!
//! Postgres migrations are forward-only. The migration that creates a table
//! renders that table. A later column is a new numbered migration with `ALTER`,
//! plus an update here. Do not rewrite a migration that has already been applied.

use rusqlite::Connection;
use sqlx::Row;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    Sqlite,
    Postgres,
}

#[derive(Clone, Copy)]
enum Kind {
    Text,
    Int,
    Bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DefaultValue {
    Zero,
    Empty,
    EmptyObject,
    False,
}

struct Column {
    name: &'static str,
    kind: Kind,
    required: bool,
    default: Option<DefaultValue>,
}

struct Index {
    name: &'static str,
    columns: &'static str,
}

pub struct Table {
    name: &'static str,
    columns: &'static [Column],
    primary_key: &'static [&'static str],
    indexes: &'static [Index],
}

const fn text(name: &'static str) -> Column {
    Column {
        name,
        kind: Kind::Text,
        required: false,
        default: None,
    }
}

const fn text_required(name: &'static str) -> Column {
    Column {
        name,
        kind: Kind::Text,
        required: true,
        default: None,
    }
}

const fn text_default(name: &'static str, default: DefaultValue) -> Column {
    Column {
        name,
        kind: Kind::Text,
        required: true,
        default: Some(default),
    }
}

const fn int(name: &'static str) -> Column {
    Column {
        name,
        kind: Kind::Int,
        required: false,
        default: None,
    }
}

const fn int_required(name: &'static str) -> Column {
    Column {
        name,
        kind: Kind::Int,
        required: true,
        default: None,
    }
}

const fn int_default(name: &'static str) -> Column {
    Column {
        name,
        kind: Kind::Int,
        required: true,
        default: Some(DefaultValue::Zero),
    }
}

const fn bool_default(name: &'static str) -> Column {
    Column {
        name,
        kind: Kind::Bool,
        required: true,
        default: Some(DefaultValue::False),
    }
}

pub static SESSIONS: Table = Table {
    name: "sessions",
    columns: &[
        text_required("org_id"),
        text_required("session_id"),
        text("hook_event"),
        text_required("created_at"),
        text_required("updated_at"),
        text("user_id"),
        text("harness"),
        text("harness_version"),
        text("model"),
        text("model_version"),
        text("client_timestamp"),
        text("prompt"),
        text_default("preview", DefaultValue::Empty),
        text("started_at"),
        text("ended_at"),
        int_default("user_turns"),
        int_default("assistant_turns"),
        int_default("tool_calls"),
        text("tool_counts"),
        bool_default("has_writes"),
        int_default("bytes"),
        text_default("view", DefaultValue::EmptyObject),
    ],
    primary_key: &["org_id", "session_id"],
    indexes: &[Index {
        name: "sessions_org_updated",
        columns: "org_id, updated_at DESC",
    }],
};

pub static BILLING: Table = Table {
    name: "billing",
    columns: &[
        text_required("org_id"),
        text_required("stripe_customer_id"),
        text_required("stripe_subscription_id"),
        text_required("status"),
        int("trial_end"),
        int("last_event_created"),
    ],
    primary_key: &["org_id"],
    indexes: &[Index {
        name: "billing_customer",
        columns: "stripe_customer_id",
    }],
};

pub static BILLING_EVENTS: Table = Table {
    name: "billing_events",
    columns: &[text_required("event_id"), int_required("created")],
    primary_key: &["event_id"],
    indexes: &[],
};

pub static SEEN_USERS: Table = Table {
    name: "seen_users",
    columns: &[text_required("user_id")],
    primary_key: &["user_id"],
    indexes: &[],
};

pub const TABLES: &[&Table] = &[&SESSIONS, &BILLING, &BILLING_EVENTS, &SEEN_USERS];

fn type_name(kind: Kind, dialect: Dialect) -> &'static str {
    match (kind, dialect) {
        (Kind::Text, _) => "TEXT",
        (Kind::Int, Dialect::Sqlite) => "INTEGER",
        (Kind::Int, Dialect::Postgres) => "BIGINT",
        (Kind::Bool, Dialect::Sqlite) => "INTEGER",
        (Kind::Bool, Dialect::Postgres) => "BOOLEAN",
    }
}

fn default_sql(default: DefaultValue, dialect: Dialect) -> &'static str {
    match (default, dialect) {
        (DefaultValue::Zero, _) => "0",
        (DefaultValue::Empty, _) => "''",
        (DefaultValue::EmptyObject, _) => "'{}'",
        (DefaultValue::False, Dialect::Sqlite) => "0",
        (DefaultValue::False, Dialect::Postgres) => "FALSE",
    }
}

fn column_sql(column: &Column, dialect: Dialect) -> String {
    let mut sql = format!("{} {}", column.name, type_name(column.kind, dialect));
    if column.required {
        sql.push_str(" NOT NULL");
    }
    if let Some(default) = column.default {
        sql.push_str(" DEFAULT ");
        sql.push_str(default_sql(default, dialect));
    }
    sql
}

/// `name TYPE [NOT NULL] [DEFAULT ...]` for an `ALTER TABLE ADD COLUMN`.
pub fn sqlite_column_clause(table: &Table, name: &str) -> String {
    let column = table
        .columns
        .iter()
        .find(|column| column.name == name)
        .unwrap_or_else(|| panic!("unknown column {name} on {}", table.name));
    column_sql(column, Dialect::Sqlite)
}

pub fn create_statements(table: &Table, dialect: Dialect, if_missing: bool) -> Vec<String> {
    let if_not = if if_missing { " IF NOT EXISTS" } else { "" };
    let columns = table
        .columns
        .iter()
        .map(|column| column_sql(column, dialect))
        .collect::<Vec<_>>()
        .join(",\n    ");
    let primary_key = table.primary_key.join(", ");
    let mut statements = vec![format!(
        "CREATE TABLE{if_not} {} (\n    {columns},\n    PRIMARY KEY ({primary_key})\n)",
        table.name
    )];
    statements.extend(index_statements(table, if_missing));
    statements
}

pub(crate) fn index_statements(table: &Table, if_missing: bool) -> Vec<String> {
    let if_not = if if_missing { " IF NOT EXISTS" } else { "" };
    table
        .indexes
        .iter()
        .map(|index| {
            format!(
                "CREATE INDEX{if_not} {} ON {} ({})",
                index.name, table.name, index.columns
            )
        })
        .collect()
}

pub fn sqlite_script(table: &Table, if_missing: bool) -> String {
    create_statements(table, Dialect::Sqlite, if_missing)
        .into_iter()
        .map(|statement| format!("{statement};"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Rewrites `?1` placeholders to `$1` for Postgres. SQLite keeps `?1`.
pub fn render(sql: &str, dialect: Dialect) -> String {
    if dialect == Dialect::Sqlite {
        return sql.to_string();
    }
    let mut out = String::with_capacity(sql.len());
    let bytes = sql.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'?' && index + 1 < bytes.len() && bytes[index + 1].is_ascii_digit() {
            out.push('$');
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                out.push(bytes[index] as char);
                index += 1;
            }
        } else {
            out.push(bytes[index] as char);
            index += 1;
        }
    }
    out
}

pub(crate) const SESSION_COLUMNS: &str = "session_id, hook_event, created_at, updated_at,
         user_id, harness, harness_version, model, model_version, client_timestamp,
         prompt, preview, started_at, ended_at, user_turns, assistant_turns, tool_calls,
         tool_counts, has_writes, bytes";

/// One upsert for both dialects. `?8` is the uploaded model, `?18` is the model
/// parsed from the transcript. A blank later upload must not wipe the harness,
/// the user, or a model already stored.
const SESSION_UPSERT: &str = "
INSERT INTO sessions (
    org_id, session_id, hook_event, created_at, updated_at,
    user_id, harness, harness_version, model, model_version,
    client_timestamp, preview, bytes, view,
    prompt, started_at, ended_at, user_turns, assistant_turns,
    tool_calls, tool_counts, has_writes
) VALUES (
    ?1, ?2, ?3, ?4, ?4,
    ?5, ?6, ?7, COALESCE(?8, ?18), ?9,
    ?10, ?11, ?12, ?13,
    ?14, ?15, ?16, ?17, ?19,
    ?20, ?21, ?22
)
ON CONFLICT (org_id, session_id) DO UPDATE SET
    hook_event = COALESCE(EXCLUDED.hook_event, sessions.hook_event),
    updated_at = EXCLUDED.updated_at,
    user_id = COALESCE(EXCLUDED.user_id, sessions.user_id),
    harness = COALESCE(EXCLUDED.harness, sessions.harness),
    harness_version = COALESCE(EXCLUDED.harness_version, sessions.harness_version),
    model = COALESCE(?8, sessions.model, ?18),
    model_version = COALESCE(EXCLUDED.model_version, sessions.model_version),
    client_timestamp = COALESCE(EXCLUDED.client_timestamp, sessions.client_timestamp),
    preview = EXCLUDED.preview,
    bytes = EXCLUDED.bytes,
    view = EXCLUDED.view,
    prompt = EXCLUDED.prompt,
    started_at = EXCLUDED.started_at,
    ended_at = EXCLUDED.ended_at,
    user_turns = EXCLUDED.user_turns,
    assistant_turns = EXCLUDED.assistant_turns,
    tool_calls = EXCLUDED.tool_calls,
    tool_counts = EXCLUDED.tool_counts,
    has_writes = EXCLUDED.has_writes
";

pub(crate) fn session_upsert(dialect: Dialect) -> String {
    render(SESSION_UPSERT, dialect)
}

pub(crate) fn session_list(dialect: Dialect) -> String {
    render(
        &format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE org_id = ?1"),
        dialect,
    )
}

pub(crate) fn session_by_id(dialect: Dialect) -> String {
    render(
        &format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE org_id = ?1 AND session_id = ?2"),
        dialect,
    )
}

pub(crate) fn session_with_view(dialect: Dialect) -> String {
    render(
        &format!(
            "SELECT {SESSION_COLUMNS}, view FROM sessions WHERE org_id = ?1 AND session_id = ?2"
        ),
        dialect,
    )
}

pub(crate) fn contributor_counts(dialect: Dialect) -> String {
    render(
        "SELECT COUNT(DISTINCT user_id) FROM sessions
         WHERE COALESCE(user_id, '') <> ''
         GROUP BY org_id",
        dialect,
    )
}

pub const BILLING_ACCOUNT: &str =
    "SELECT stripe_customer_id, stripe_subscription_id, status, trial_end
         FROM billing WHERE org_id = ?1";

pub const BILLING_STATUS: &str = "SELECT status FROM billing WHERE org_id = ?1";

pub const BILLING_SAVE: &str =
    "UPDATE billing SET stripe_subscription_id = ?1, status = ?2, trial_end = ?3 WHERE org_id = ?4";

pub const BILLING_EVENT_SEEN: &str = "SELECT 1 FROM billing_events WHERE event_id = ?1";

pub const BILLING_BY_CUSTOMER: &str =
    "SELECT org_id, status FROM billing WHERE stripe_customer_id = ?1";

pub const BILLING_EVENT_INSERT: &str =
    "INSERT INTO billing_events (event_id, created) VALUES (?1, ?2)";

pub const BILLING_APPLY: &str = "UPDATE billing
         SET status = ?1, trial_end = ?2, stripe_subscription_id = ?3, last_event_created = ?4
         WHERE stripe_customer_id = ?5
           AND (last_event_created IS NULL OR last_event_created <= ?4)";

pub const BILLING_EVENT_DELETE_OLD: &str = "DELETE FROM billing_events WHERE created < ?1";

pub const BILLING_STATUS_COUNTS: &str = "SELECT status, COUNT(*) FROM billing GROUP BY status";

pub fn insert_billing_account(dialect: Dialect) -> String {
    insert_ignore(
        "billing",
        "org_id, stripe_customer_id, stripe_subscription_id, status, trial_end",
        "?1, ?2, ?3, ?4, ?5",
        "org_id",
        dialect,
    )
}

pub fn insert_billing_event_once(dialect: Dialect) -> String {
    insert_ignore(
        "billing_events",
        "event_id, created",
        "?1, ?2",
        "event_id",
        dialect,
    )
}

pub fn insert_seen_user(dialect: Dialect) -> String {
    insert_ignore("seen_users", "user_id", "?1", "user_id", dialect)
}

fn insert_ignore(
    table: &str,
    columns: &str,
    values: &str,
    conflict: &str,
    dialect: Dialect,
) -> String {
    let sql = match dialect {
        Dialect::Sqlite => {
            format!("INSERT OR IGNORE INTO {table} ({columns}) VALUES ({values})")
        }
        Dialect::Postgres => format!(
            "INSERT INTO {table} ({columns}) VALUES ({values}) ON CONFLICT ({conflict}) DO NOTHING"
        ),
    };
    render(&sql, dialect)
}

struct LiveColumn {
    name: String,
    ty: String,
    required: bool,
    default: Option<String>,
    pk_position: i64,
}

pub fn check_sqlite(connection: &Connection, table: &Table) -> Result<(), String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({})", table.name))
        .map_err(|err| err.to_string())?;
    let live = statement
        .query_map([], |row| {
            Ok(LiveColumn {
                name: row.get(1)?,
                ty: row.get(2)?,
                required: row.get::<_, i64>(3)? == 1,
                default: row.get(4)?,
                pk_position: row.get(5)?,
            })
        })
        .map_err(|err| err.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| err.to_string())?;
    compare(table, Dialect::Sqlite, &live)?;
    for index in table.indexes {
        let found: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                [index.name],
                |row| row.get(0),
            )
            .map_err(|err| err.to_string())?;
        if found == 0 {
            return Err(format!("{} is missing index {}", table.name, index.name));
        }
    }
    Ok(())
}

pub(crate) async fn check_postgres(pool: &sqlx::PgPool) -> Result<(), String> {
    for table in TABLES {
        let rows = sqlx::query(
            "SELECT column_name, data_type, is_nullable, column_default
             FROM information_schema.columns
             WHERE table_schema = 'public' AND table_name = $1
             ORDER BY ordinal_position",
        )
        .bind(table.name)
        .fetch_all(pool)
        .await
        .map_err(|err| err.to_string())?;
        let live = rows
            .iter()
            .map(|row| {
                Ok(LiveColumn {
                    name: row.try_get(0)?,
                    ty: row.try_get(1)?,
                    required: row.try_get::<String, _>(2)? == "NO",
                    default: row.try_get(3)?,
                    pk_position: 0,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()
            .map_err(|err| err.to_string())?;
        let keys = sqlx::query(
            "SELECT kcu.column_name
             FROM information_schema.table_constraints tc
             JOIN information_schema.key_column_usage kcu
               ON tc.constraint_name = kcu.constraint_name
              AND tc.table_schema = kcu.table_schema
              AND tc.table_name = kcu.table_name
             WHERE tc.table_schema = 'public'
               AND tc.table_name = $1
               AND tc.constraint_type = 'PRIMARY KEY'
             ORDER BY kcu.ordinal_position",
        )
        .bind(table.name)
        .fetch_all(pool)
        .await
        .map_err(|err| err.to_string())?;
        let mut live = live;
        for (position, key) in keys.iter().enumerate() {
            let name: String = key.try_get(0).map_err(|err| err.to_string())?;
            if let Some(column) = live.iter_mut().find(|column| column.name == name) {
                column.pk_position = (position as i64) + 1;
            }
        }
        compare(table, Dialect::Postgres, &live)?;
        for index in table.indexes {
            let found: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM pg_indexes
                 WHERE schemaname = 'public' AND indexname = $1",
            )
            .bind(index.name)
            .fetch_one(pool)
            .await
            .map_err(|err| err.to_string())?;
            if found == 0 {
                return Err(format!("{} is missing index {}", table.name, index.name));
            }
        }
    }
    Ok(())
}

fn compare(table: &Table, dialect: Dialect, live: &[LiveColumn]) -> Result<(), String> {
    if live.is_empty() {
        return Err(format!("table {} does not exist", table.name));
    }
    let mut missing: Vec<&str> = table.columns.iter().map(|column| column.name).collect();
    for column in live {
        let Some(expected) = table
            .columns
            .iter()
            .find(|expected| expected.name == column.name)
        else {
            return Err(format!(
                "{} has unexpected column {}",
                table.name, column.name
            ));
        };
        missing.retain(|name| *name != column.name);
        let want_type = type_name(expected.kind, dialect);
        if !column.ty.eq_ignore_ascii_case(want_type) {
            return Err(format!(
                "{}.{} is {}, shared schema says {want_type}",
                table.name, column.name, column.ty
            ));
        }
        if column.required != expected.required {
            return Err(format!(
                "{}.{} nullability does not match the shared schema",
                table.name, column.name
            ));
        }
        let want_default = expected
            .default
            .map(|default| normalize_default(default_sql(default, dialect)));
        let got_default = column.default.as_deref().map(normalize_default);
        if want_default != got_default {
            return Err(format!(
                "{}.{} default is {got_default:?}, shared schema says {want_default:?}",
                table.name, column.name
            ));
        }
    }
    if let Some(name) = missing.first() {
        return Err(format!("{} is missing column {name}", table.name));
    }
    let mut primary_key: Vec<&LiveColumn> = live
        .iter()
        .filter(|column| column.pk_position > 0)
        .collect();
    primary_key.sort_by_key(|column| column.pk_position);
    let names: Vec<&str> = primary_key
        .iter()
        .map(|column| column.name.as_str())
        .collect();
    if names != table.primary_key {
        return Err(format!(
            "{} primary key is {names:?}, shared schema says {:?}",
            table.name, table.primary_key
        ));
    }
    Ok(())
}

fn normalize_default(value: &str) -> String {
    let value = value.trim();
    let value = value.split("::").next().unwrap_or(value).trim();
    value.to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_dialect_specific() {
        let sql = "WHERE id = ?1 AND n = ?12";
        assert_eq!(render(sql, Dialect::Sqlite), sql);
        assert_eq!(render(sql, Dialect::Postgres), "WHERE id = $1 AND n = $12");
    }

    #[test]
    fn both_dialects_run_the_same_session_upsert() {
        let sqlite = session_upsert(Dialect::Sqlite);
        assert_eq!(
            render(&sqlite, Dialect::Postgres),
            session_upsert(Dialect::Postgres)
        );
        assert!(sqlite.contains("COALESCE(?8, sessions.model, ?18)"));
    }

    #[test]
    fn rendered_tables_match_the_sqlite_catalog() {
        let connection = Connection::open_in_memory().unwrap();
        for table in TABLES {
            connection
                .execute_batch(&sqlite_script(table, false))
                .unwrap();
            check_sqlite(&connection, table).unwrap();
        }
    }
}
