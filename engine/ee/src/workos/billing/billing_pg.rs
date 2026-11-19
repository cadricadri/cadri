//! Billing statements for the hosted Postgres pool.
//! The SQL text lives in `cadri_core::schema`, shared with SQLite.

use cadri_core::schema::{self, Dialect};
use cadri_core::ApiError;
use sqlx::{PgPool, Row};

use super::{db_error_message, Account};

fn statement(sql: &str) -> String {
    schema::render(sql, Dialect::Postgres)
}

async fn timed<T>(
    op: &'static str,
    future: impl std::future::Future<Output = Result<T, sqlx::Error>>,
) -> Result<T, ApiError> {
    cadri_core::time_postgres(op, future)
        .await
        .map_err(db_error_message)
}

pub(super) async fn account(pool: &PgPool, org_id: &str) -> Result<Option<Account>, ApiError> {
    let sql = statement(schema::BILLING_ACCOUNT);
    let row = timed(
        "billing",
        sqlx::query(&sql).bind(org_id).fetch_optional(pool),
    )
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(Account {
        stripe_customer_id: row.try_get(0).map_err(db_error_message)?,
        stripe_subscription_id: row.try_get(1).map_err(db_error_message)?,
        status: row.try_get(2).map_err(db_error_message)?,
        trial_end: row.try_get(3).map_err(db_error_message)?,
    }))
}

pub(super) async fn insert_account(
    pool: &PgPool,
    org_id: &str,
    account: &Account,
) -> Result<u64, ApiError> {
    let sql = schema::insert_billing_account(Dialect::Postgres);
    let inserted = timed(
        "billing",
        sqlx::query(&sql)
            .bind(org_id)
            .bind(&account.stripe_customer_id)
            .bind(&account.stripe_subscription_id)
            .bind(&account.status)
            .bind(account.trial_end)
            .execute(pool),
    )
    .await?;
    Ok(inserted.rows_affected())
}

pub(super) async fn previous_status(
    pool: &PgPool,
    org_id: &str,
) -> Result<Option<String>, ApiError> {
    let sql = statement(schema::BILLING_STATUS);
    timed(
        "billing",
        sqlx::query_scalar(&sql).bind(org_id).fetch_optional(pool),
    )
    .await
}

pub(super) async fn save_account(
    pool: &PgPool,
    org_id: &str,
    account: &Account,
) -> Result<(), ApiError> {
    let sql = statement(schema::BILLING_SAVE);
    timed(
        "billing",
        sqlx::query(&sql)
            .bind(&account.stripe_subscription_id)
            .bind(&account.status)
            .bind(account.trial_end)
            .bind(org_id)
            .execute(pool),
    )
    .await?;
    Ok(())
}

pub(super) enum ApplyResult {
    Duplicate,
    Ignored,
    Applied {
        org_id: String,
        from: String,
        status: String,
    },
}

pub(super) async fn apply_subscription_event(
    pool: &PgPool,
    event_id: &str,
    created: i64,
    customer_id: &str,
    status: &str,
    trial_end: Option<i64>,
    subscription_id: &str,
) -> Result<ApplyResult, ApiError> {
    let mut transaction = timed("billing", pool.begin()).await?;
    let seen_sql = statement(schema::BILLING_EVENT_SEEN);
    let seen: Option<i32> = timed(
        "billing",
        sqlx::query_scalar(&seen_sql)
            .bind(event_id)
            .fetch_optional(&mut *transaction),
    )
    .await?;
    if seen.is_some() {
        return Ok(ApplyResult::Duplicate);
    }
    let customer_sql = statement(schema::BILLING_BY_CUSTOMER);
    let existing: Option<(String, String)> = timed(
        "billing",
        sqlx::query_as(&customer_sql)
            .bind(customer_id)
            .fetch_optional(&mut *transaction),
    )
    .await?;
    let Some((org_id, previous)) = existing else {
        record_event(&mut transaction, event_id, created).await?;
        timed("billing", transaction.commit()).await?;
        eprintln!("Stripe customer {customer_id} does not match a Cadri team");
        return Ok(ApplyResult::Ignored);
    };
    let apply_sql = statement(schema::BILLING_APPLY);
    let changed = timed(
        "billing",
        sqlx::query(&apply_sql)
            .bind(status)
            .bind(trial_end)
            .bind(subscription_id)
            .bind(created)
            .bind(customer_id)
            .execute(&mut *transaction),
    )
    .await?;
    if changed.rows_affected() == 0 {
        record_event(&mut transaction, event_id, created).await?;
        timed("billing", transaction.commit()).await?;
        return Ok(ApplyResult::Ignored);
    }
    timed("billing", transaction.commit()).await?;
    Ok(ApplyResult::Applied {
        org_id,
        from: previous,
        status: status.to_string(),
    })
}

pub(super) async fn event_seen(pool: &PgPool, event_id: &str) -> Result<bool, ApiError> {
    let sql = statement(schema::BILLING_EVENT_SEEN);
    let seen: Option<i32> = timed(
        "billing",
        sqlx::query_scalar(&sql).bind(event_id).fetch_optional(pool),
    )
    .await?;
    Ok(seen.is_some())
}

pub(super) async fn mark_event(
    pool: &PgPool,
    event_id: &str,
    created: i64,
) -> Result<(), ApiError> {
    let insert_sql = schema::insert_billing_event_once(Dialect::Postgres);
    timed(
        "billing",
        sqlx::query(&insert_sql)
            .bind(event_id)
            .bind(created)
            .execute(pool),
    )
    .await?;
    let cutoff = crate::stripe::unix_now() - 30 * 24 * 60 * 60;
    let delete_sql = statement(schema::BILLING_EVENT_DELETE_OLD);
    timed(
        "billing",
        sqlx::query(&delete_sql).bind(cutoff).execute(pool),
    )
    .await?;
    Ok(())
}

pub(super) async fn status_counts(pool: &PgPool) -> Result<Vec<(String, i64)>, ApiError> {
    let sql = statement(schema::BILLING_STATUS_COUNTS);
    timed("billing", sqlx::query_as(&sql).fetch_all(pool)).await
}

pub(super) async fn note_signup(pool: &PgPool, user_id: &str) -> Result<bool, ApiError> {
    let sql = schema::insert_seen_user(Dialect::Postgres);
    let inserted = timed("signup", sqlx::query(&sql).bind(user_id).execute(pool)).await?;
    Ok(inserted.rows_affected() == 1)
}

async fn record_event(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event_id: &str,
    created: i64,
) -> Result<(), ApiError> {
    let sql = statement(schema::BILLING_EVENT_INSERT);
    timed(
        "billing",
        sqlx::query(&sql)
            .bind(event_id)
            .bind(created)
            .execute(&mut **transaction),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn one_event_applied_twice_is_a_duplicate() {
        let Ok(url) = std::env::var("CADRI_DATABASE_URL") else {
            if std::env::var("CADRI_REQUIRE_POSTGRES").ok().as_deref() == Some("1") {
                panic!("CADRI_REQUIRE_POSTGRES=1 but CADRI_DATABASE_URL is empty");
            }
            return;
        };
        if url.trim().is_empty() {
            return;
        }
        let database = cadri_core::Database::connect_postgres(url.trim())
            .await
            .expect("postgres");
        let pool = database.pg_pool().expect("pool");
        let org = format!("pg-bill-{}", std::process::id());
        let customer = format!("cus_{org}");
        let event_id = format!("evt_{org}");
        sqlx::query("DELETE FROM billing WHERE org_id = $1")
            .bind(&org)
            .execute(pool)
            .await
            .expect("cleanup account");
        let account = Account {
            stripe_customer_id: customer.clone(),
            stripe_subscription_id: "sub_1".into(),
            status: "trialing".into(),
            trial_end: Some(1_800_000_000),
        };
        assert_eq!(
            insert_account(pool, &org, &account).await.expect("insert"),
            1
        );
        let created = 1_800_000_000_i64;
        let first =
            apply_subscription_event(pool, &event_id, created, &customer, "active", None, "sub_1")
                .await
                .expect("apply");
        assert!(matches!(first, ApplyResult::Applied { status, .. } if status == "active"));
        mark_event(pool, &event_id, created)
            .await
            .expect("record event");
        let second =
            apply_subscription_event(pool, &event_id, created, &customer, "active", None, "sub_1")
                .await
                .expect("replay");
        assert!(matches!(second, ApplyResult::Duplicate));
        sqlx::query("DELETE FROM billing_events WHERE event_id = $1")
            .bind(&event_id)
            .execute(pool)
            .await
            .expect("cleanup event");
        sqlx::query("DELETE FROM billing WHERE org_id = $1")
            .bind(&org)
            .execute(pool)
            .await
            .expect("cleanup account");
    }
}
