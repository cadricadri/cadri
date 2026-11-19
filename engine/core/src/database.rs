//! SQLite by default. Postgres when `CADRI_DATABASE_URL` is set (hosted engine).

use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use sqlx::PgPool;

use crate::db::{self, IncomingSession, SessionRecord, StoredSession};
use crate::pg;

/// One open engine database. Cheap to clone: SQLite is an `Arc`, Postgres is a pool handle.
#[derive(Clone)]
pub struct Database {
    kind: Kind,
}

#[derive(Clone)]
enum Kind {
    Sqlite(Arc<Mutex<Connection>>),
    Postgres(PgPool),
}

impl Database {
    pub fn sqlite(connection: Connection) -> Self {
        Self {
            kind: Kind::Sqlite(Arc::new(Mutex::new(connection))),
        }
    }

    /// Connects, migrates forward, and returns a pool of 8 connections.
    pub async fn connect_postgres(url: &str) -> Result<Self, String> {
        let pool = pg::connect(url).await?;
        Ok(Self {
            kind: Kind::Postgres(pool),
        })
    }

    pub fn is_postgres(&self) -> bool {
        matches!(self.kind, Kind::Postgres(_))
    }

    pub fn sqlite_connection(&self) -> Option<Arc<Mutex<Connection>>> {
        match &self.kind {
            Kind::Sqlite(connection) => Some(Arc::clone(connection)),
            Kind::Postgres(_) => None,
        }
    }

    pub fn pg_pool(&self) -> Option<&PgPool> {
        match &self.kind {
            Kind::Postgres(pool) => Some(pool),
            Kind::Sqlite(_) => None,
        }
    }

    pub(crate) async fn check(&self) -> Result<(), String> {
        match &self.kind {
            Kind::Sqlite(connection) => {
                let connection = lock(connection)?;
                db::check(&connection).map_err(|err| err.to_string())
            }
            Kind::Postgres(pool) => pg::check(pool).await,
        }
    }

    pub(crate) async fn upsert(
        &self,
        org_id: &str,
        user_id: &str,
        incoming: &IncomingSession,
    ) -> Result<SessionRecord, String> {
        match &self.kind {
            Kind::Sqlite(connection) => {
                let connection = lock(connection)?;
                db::upsert(&connection, org_id, user_id, incoming).map_err(|err| err.to_string())
            }
            Kind::Postgres(pool) => pg::upsert(pool, org_id, user_id, incoming).await,
        }
    }

    pub(crate) async fn list(&self, org_id: &str) -> Result<Vec<SessionRecord>, String> {
        match &self.kind {
            Kind::Sqlite(connection) => {
                let connection = lock(connection)?;
                db::list(&connection, org_id).map_err(|err| err.to_string())
            }
            Kind::Postgres(pool) => pg::list(pool, org_id).await,
        }
    }

    pub(crate) async fn get(
        &self,
        org_id: &str,
        session_id: &str,
    ) -> Result<Option<StoredSession>, String> {
        match &self.kind {
            Kind::Sqlite(connection) => {
                let connection = lock(connection)?;
                db::get(&connection, org_id, session_id).map_err(|err| err.to_string())
            }
            Kind::Postgres(pool) => pg::get(pool, org_id, session_id).await,
        }
    }

    pub(crate) async fn contributor_counts(&self) -> Result<Vec<i64>, String> {
        match &self.kind {
            Kind::Sqlite(connection) => {
                let connection = lock(connection)?;
                let mut statement = connection
                    .prepare(&crate::schema::contributor_counts(
                        crate::schema::Dialect::Sqlite,
                    ))
                    .map_err(|err| err.to_string())?;
                let rows = statement
                    .query_map([], |row| row.get::<_, i64>(0))
                    .map_err(|err| err.to_string())?;
                rows.collect::<Result<Vec<_>, _>>()
                    .map_err(|err| err.to_string())
            }
            Kind::Postgres(pool) => pg::contributor_counts(pool).await,
        }
    }
}

fn lock(connection: &Mutex<Connection>) -> Result<std::sync::MutexGuard<'_, Connection>, String> {
    connection
        .lock()
        .map_err(|_| "database lock poisoned".to_string())
}
