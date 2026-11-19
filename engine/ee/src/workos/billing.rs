use std::sync::Mutex;

use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::PrivateCookieJar;
use cadri_core::schema::{self, Dialect};
use cadri_core::{ApiError, Identity};
use hmac::{Hmac, Mac};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::Value;
use sha2::Sha256;
use subtle::ConstantTimeEq;

use super::{read_session, session_cookie, WorkOs};

mod billing_pg;
use crate::stripe::{self, StripeError, Subscription};

const ENTITLEMENT: &str = "cadri";

type HmacSha256 = Hmac<Sha256>;

pub(crate) struct Account {
    pub(crate) stripe_customer_id: String,
    pub(crate) stripe_subscription_id: String,
    pub(crate) status: String,
    pub(crate) trial_end: Option<i64>,
}

#[derive(serde::Serialize)]
pub(crate) struct Summary {
    pub status: String,
    pub trial_end: Option<i64>,
    pub seat_count: Option<u64>,
    pub access: bool,
}

#[derive(Deserialize)]
struct Event {
    id: String,
    #[serde(default)]
    created: i64,
    #[serde(default)]
    api_version: Option<String>,
    #[serde(rename = "type")]
    kind: String,
    data: EventData,
}

enum EventWrite {
    Duplicate,
    Ignored,
    Applied {
        org_id: String,
        from: String,
        status: String,
    },
}

#[derive(Deserialize)]
struct EventData {
    object: Value,
}

impl WorkOs {
    pub(crate) fn billing_enabled(&self) -> bool {
        self.stripe.is_some()
    }

    pub(crate) fn prepare_billing(&self, db: &cadri_core::Database) {
        if self.stripe.is_none() {
            return;
        }
        let Some(sqlite) = db.sqlite_connection() else {
            let _ = self.db.set(db.clone());
            return;
        };
        let connection = sqlite
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let script = format!(
            "{}\n{}",
            schema::sqlite_script(&schema::BILLING, true),
            schema::sqlite_script(&schema::BILLING_EVENTS, true),
        );
        if let Err(err) = connection.execute_batch(&script) {
            eprintln!("billing table setup failed: {err}");
            return;
        }
        // Databases created before last_event_created existed.
        let column = schema::sqlite_column_clause(&schema::BILLING, "last_event_created");
        if let Err(err) =
            connection.execute(&format!("ALTER TABLE billing ADD COLUMN {column}"), [])
        {
            if !err.to_string().contains("duplicate column") {
                eprintln!("billing table setup failed: {err}");
                return;
            }
        }
        if let Err(err) = schema::check_sqlite(&connection, &schema::BILLING)
            .and_then(|_| schema::check_sqlite(&connection, &schema::BILLING_EVENTS))
        {
            eprintln!("billing table setup failed: {err}");
            return;
        }
        drop(connection);
        let _ = self.db.set(db.clone());
    }

    /// Creates the Stripe customer and the 30-day trial once per organization.
    pub(crate) async fn ensure_account(&self, org_id: &str, user_id: &str) -> Result<(), ApiError> {
        let Some(config) = &self.stripe else {
            return Ok(());
        };
        if self.account(org_id).await?.is_some() {
            return Ok(());
        }
        let (name, email) = tokio::join!(
            self.profile_field(format!("/organizations/{org_id}"), "name"),
            self.profile_field(format!("/user_management/users/{user_id}"), "email"),
        );
        let name = name.unwrap_or_else(|| org_id.to_string());
        let customer_id =
            stripe::create_customer(&self.http, config, org_id, &name, email.as_deref(), user_id)
                .await
                .map_err(stripe_error)?;
        let subscription =
            stripe::create_trial_subscription(&self.http, config, org_id, &customer_id)
                .await
                .map_err(stripe_error)?;
        self.set_stripe_customer(org_id, &customer_id, &name)
            .await
            .map_err(super::workos_call)?;
        self.insert_account(
            org_id,
            &Account {
                stripe_customer_id: customer_id,
                stripe_subscription_id: subscription.id,
                status: subscription.status,
                trial_end: subscription.trial_end,
            },
        )
        .await
    }

    pub(crate) async fn access_allowed(
        &self,
        entitlements: &[String],
        identity: &Identity,
    ) -> Result<bool, ApiError> {
        if !self.billing_enabled() {
            return Ok(true);
        }
        if self
            .config
            .billing_allowlist_org_ids
            .iter()
            .any(|id| id == &identity.org_id)
        {
            return Ok(true);
        }
        if let Some(account) = self.account(&identity.org_id).await? {
            if blocks_access(&account.status) {
                return Ok(false);
            }
            if trial_open(&account) {
                return Ok(true);
            }
        }
        if entitlements.iter().any(|item| item == ENTITLEMENT) {
            return Ok(true);
        }
        self.ensure_account(&identity.org_id, &identity.user_id)
            .await?;
        let Some(account) = self.account(&identity.org_id).await? else {
            return Ok(false);
        };
        Ok(trial_open(&account))
    }

    pub(crate) async fn billing_summary(
        &self,
        identity: &Identity,
        entitlements: &[String],
        headers: &HeaderMap,
        portal_return: bool,
    ) -> Result<(Summary, HeaderMap), ApiError> {
        let config = self.stripe.as_ref().ok_or_else(billing_disabled)?;
        self.ensure_account(&identity.org_id, &identity.user_id)
            .await?;
        let mut account = self
            .account(&identity.org_id)
            .await?
            .ok_or_else(billing_disabled)?;
        let mut subscription =
            stripe::get_subscription(&self.http, config, &account.stripe_subscription_id)
                .await
                .map_err(stripe_error)?;
        let mut refreshed = HeaderMap::new();
        if portal_return && self.has_card(config, &account, &subscription).await? {
            if subscription.status == "paused" {
                // Legacy pause-at-trial-end rows (pre metered+cancel fix) still resume.
                subscription = stripe::resume_subscription(
                    &self.http,
                    config,
                    &account.stripe_subscription_id,
                )
                .await
                .map_err(stripe_error)?;
                match self.refresh_cookie(headers).await {
                    Ok(Some(cookie)) => refreshed = cookie,
                    Ok(None) => {}
                    Err(err) => {
                        eprintln!("could not refresh the session after resuming billing: {err:?}");
                    }
                }
            } else if subscription.status == "canceled" {
                // Metered trials cancel when no card is present; start a new paid sub.
                subscription = stripe::create_paid_subscription(
                    &self.http,
                    config,
                    &identity.org_id,
                    &account.stripe_customer_id,
                )
                .await
                .map_err(stripe_error)?;
                account.stripe_subscription_id = subscription.id.clone();
                match self.refresh_cookie(headers).await {
                    Ok(Some(cookie)) => refreshed = cookie,
                    Ok(None) => {}
                    Err(err) => {
                        eprintln!(
                            "could not refresh the session after starting paid billing: {err:?}"
                        );
                    }
                }
            }
        }
        account.status = subscription.status;
        account.trial_end = subscription.trial_end;
        self.save_account(&identity.org_id, &account).await?;
        let seat_count = match self.active_member_count(&identity.org_id).await {
            Ok(count) => Some(count),
            Err(err) => {
                eprintln!("could not count seats for {}: {err:?}", identity.org_id);
                None
            }
        };
        let entitled = entitlements.iter().any(|item| item == ENTITLEMENT);
        Ok((
            Summary {
                access: account_access(entitled, &account),
                status: account.status,
                trial_end: account.trial_end,
                seat_count,
            },
            refreshed,
        ))
    }

    pub(crate) async fn billing_portal(&self, identity: &Identity) -> Result<String, ApiError> {
        let config = self.stripe.as_ref().ok_or_else(billing_disabled)?;
        self.ensure_account(&identity.org_id, &identity.user_id)
            .await?;
        let account = self
            .account(&identity.org_id)
            .await?
            .ok_or_else(billing_disabled)?;
        let return_url = format!("{}/billing?portal_return=1", self.config.public_url);
        stripe::portal_url(&self.http, config, &account.stripe_customer_id, &return_url)
            .await
            .map_err(stripe_error)
    }

    /// Invoices for the org's Stripe customer. No billing row yet is an empty history.
    pub(crate) async fn billing_history(
        &self,
        identity: &Identity,
    ) -> Result<Vec<stripe::BillingInvoice>, ApiError> {
        let config = self.stripe.as_ref().ok_or_else(billing_disabled)?;
        let Some(account) = self.account(&identity.org_id).await? else {
            return Ok(Vec::new());
        };
        stripe::list_invoices(&self.http, config, &account.stripe_customer_id)
            .await
            .map_err(stripe_error)
    }

    pub(crate) async fn handle_stripe_webhook(
        &self,
        payload: &[u8],
        signature: &str,
    ) -> Result<(), ApiError> {
        let config = self.stripe.as_ref().ok_or_else(billing_disabled)?;
        if !signature_is_valid(
            payload,
            signature,
            &config.webhook_secret,
            stripe::unix_now(),
        ) {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid_signature"));
        }
        let event: Event = serde_json::from_slice(payload)
            .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "invalid_payload"))?;
        if event
            .api_version
            .as_deref()
            .is_some_and(|version| version != stripe::API_VERSION)
        {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "unexpected_api_version",
            ));
        }
        if event.kind == "invoice.payment_failed" {
            return self.payment_failed(config, &event).await;
        }
        if !matches!(
            event.kind.as_str(),
            "customer.subscription.updated"
                | "customer.subscription.resumed"
                | "customer.subscription.paused"
                | "customer.subscription.deleted"
        ) {
            return Ok(());
        }
        let customer_id = customer_id(&event)?;
        let subscription: Subscription = serde_json::from_value(event.data.object.clone())
            .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "invalid_payload"))?;
        self.apply_and_report(config, &event, &customer_id, &subscription)
            .await
    }

    /// When Stripe has no further payment attempt scheduled, cancel the subscription so the
    /// `cadri` feature drops. `past_due` and `canceled` already lock access even if the token
    /// still lists the entitlement.
    async fn payment_failed(
        &self,
        config: &crate::stripe::StripeConfig,
        event: &Event,
    ) -> Result<(), ApiError> {
        if self.event_seen(&event.id).await? {
            return Ok(());
        }
        let customer_id = customer_id(event)?;
        if !retries_exhausted(&event.data.object) {
            self.mark_event(&event.id, event.created).await?;
            return Ok(());
        }
        let Some(subscription_id) = event
            .data
            .object
            .get("subscription")
            .and_then(Value::as_str)
        else {
            self.mark_event(&event.id, event.created).await?;
            return Ok(());
        };
        let subscription = stripe::cancel_subscription(&self.http, config, subscription_id)
            .await
            .map_err(stripe_error)?;
        self.apply_and_report(config, event, &customer_id, &subscription)
            .await
    }

    async fn apply_and_report(
        &self,
        config: &crate::stripe::StripeConfig,
        event: &Event,
        customer_id: &str,
        subscription: &Subscription,
    ) -> Result<(), ApiError> {
        match self
            .apply_subscription_event(event, customer_id, subscription)
            .await?
        {
            EventWrite::Duplicate => Ok(()),
            EventWrite::Ignored => Ok(()),
            EventWrite::Applied {
                org_id,
                from,
                status,
            } => {
                cadri_core::observe_billing_transition(&from, &status);
                if status == "active" {
                    let seats = self
                        .active_member_count(&org_id)
                        .await
                        .map_err(super::workos_call)?;
                    stripe::report_seat_count(&self.http, config, customer_id, seats)
                        .await
                        .map_err(stripe_error)?;
                }
                self.mark_event(&event.id, event.created).await
            }
        }
    }

    async fn has_card(
        &self,
        config: &crate::stripe::StripeConfig,
        account: &Account,
        subscription: &Subscription,
    ) -> Result<bool, ApiError> {
        if subscription
            .default_payment_method
            .as_ref()
            .is_some_and(|method| !method.is_empty())
        {
            return Ok(true);
        }
        stripe::customer_has_payment_method(&self.http, config, &account.stripe_customer_id)
            .await
            .map_err(stripe_error)
    }

    async fn refresh_cookie(&self, headers: &HeaderMap) -> Result<Option<HeaderMap>, ApiError> {
        let jar = self.cookie_jar(headers);
        let Some(session) = read_session(&jar) else {
            return Ok(None);
        };
        let fresh = self.refresh_session(&session.refresh_token).await?;
        let jar = PrivateCookieJar::new(self.config.cookie_key.clone()).add(session_cookie(&fresh));
        Ok(Some(jar.into_response().into_parts().0.headers))
    }

    async fn account(&self, org_id: &str) -> Result<Option<Account>, ApiError> {
        let Some(db) = self.db.get() else {
            return Err(billing_disabled());
        };
        if let Some(pool) = db.pg_pool() {
            return billing_pg::account(pool, org_id).await;
        }
        self.with_db(|connection| {
            connection
                .query_row(schema::BILLING_ACCOUNT, [org_id], |row| {
                    Ok(Account {
                        stripe_customer_id: row.get(0)?,
                        stripe_subscription_id: row.get(1)?,
                        status: row.get(2)?,
                        trial_end: row.get(3)?,
                    })
                })
                .optional()
        })
    }

    async fn insert_account(&self, org_id: &str, account: &Account) -> Result<(), ApiError> {
        let Some(db) = self.db.get() else {
            return Err(billing_disabled());
        };
        if let Some(pool) = db.pg_pool() {
            if billing_pg::insert_account(pool, org_id, account).await? == 1 {
                cadri_core::observe_billing_transition("none", &account.status);
            }
            return Ok(());
        }
        let Some(sqlite) = db.sqlite_connection() else {
            return Err(billing_disabled());
        };
        let connection = lock_db(&sqlite)?;
        match connection.execute(
            &schema::insert_billing_account(Dialect::Sqlite),
            params![
                org_id,
                account.stripe_customer_id,
                account.stripe_subscription_id,
                account.status,
                account.trial_end,
            ],
        ) {
            Ok(changed) => {
                if changed == 1 {
                    cadri_core::observe_billing_transition("none", &account.status);
                }
                Ok(())
            }
            Err(err)
                if err.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation) =>
            {
                Ok(())
            }
            Err(err) => db_error(err),
        }
    }

    async fn save_account(&self, org_id: &str, account: &Account) -> Result<(), ApiError> {
        let Some(db) = self.db.get() else {
            return Err(billing_disabled());
        };
        if let Some(pool) = db.pg_pool() {
            let previous = billing_pg::previous_status(pool, org_id).await?;
            billing_pg::save_account(pool, org_id, account).await?;
            if let Some(previous) = previous {
                if previous != account.status {
                    cadri_core::observe_billing_transition(&previous, &account.status);
                }
            }
            return Ok(());
        }
        self.with_db(|connection| {
            let previous: Option<String> = connection
                .query_row(schema::BILLING_STATUS, [org_id], |row| row.get(0))
                .optional()?;
            connection.execute(
                schema::BILLING_SAVE,
                params![
                    account.stripe_subscription_id,
                    account.status,
                    account.trial_end,
                    org_id,
                ],
            )?;
            if let Some(previous) = previous {
                if previous != account.status {
                    cadri_core::observe_billing_transition(&previous, &account.status);
                }
            }
            Ok(())
        })
    }

    /// Applies a subscription snapshot only when this event is newer than the last one stored
    /// for the customer. A repeated event id is ignored. The event id is recorded after side
    /// effects, so a failed seat-count report is retried.
    async fn apply_subscription_event(
        &self,
        event: &Event,
        customer_id: &str,
        subscription: &Subscription,
    ) -> Result<EventWrite, ApiError> {
        let Some(db) = self.db.get() else {
            return Err(billing_disabled());
        };
        if let Some(pool) = db.pg_pool() {
            return match billing_pg::apply_subscription_event(
                pool,
                &event.id,
                event.created,
                customer_id,
                &subscription.status,
                subscription.trial_end,
                &subscription.id,
            )
            .await?
            {
                billing_pg::ApplyResult::Duplicate => Ok(EventWrite::Duplicate),
                billing_pg::ApplyResult::Ignored => Ok(EventWrite::Ignored),
                billing_pg::ApplyResult::Applied {
                    org_id,
                    from,
                    status,
                } => Ok(EventWrite::Applied {
                    org_id,
                    from,
                    status,
                }),
            };
        }
        self.with_db(|connection| {
            let transaction = connection.unchecked_transaction()?;
            let seen = transaction
                .query_row(schema::BILLING_EVENT_SEEN, [&event.id], |_| Ok(true))
                .optional()?
                .is_some();
            if seen {
                return Ok(EventWrite::Duplicate);
            }
            let existing: Option<(String, String)> = transaction
                .query_row(schema::BILLING_BY_CUSTOMER, [customer_id], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .optional()?;
            let Some((org_id, previous)) = existing else {
                transaction.execute(
                    schema::BILLING_EVENT_INSERT,
                    params![event.id, event.created],
                )?;
                transaction.commit()?;
                eprintln!("Stripe customer {customer_id} does not match a Cadri team");
                return Ok(EventWrite::Ignored);
            };
            let changed = transaction.execute(
                schema::BILLING_APPLY,
                params![
                    subscription.status,
                    subscription.trial_end,
                    subscription.id,
                    event.created,
                    customer_id,
                ],
            )?;
            if changed == 0 {
                transaction.execute(
                    schema::BILLING_EVENT_INSERT,
                    params![event.id, event.created],
                )?;
                transaction.commit()?;
                return Ok(EventWrite::Ignored);
            }
            transaction.commit()?;
            Ok(EventWrite::Applied {
                org_id,
                from: previous,
                status: subscription.status.clone(),
            })
        })
    }

    async fn event_seen(&self, event_id: &str) -> Result<bool, ApiError> {
        let Some(db) = self.db.get() else {
            return Err(billing_disabled());
        };
        if let Some(pool) = db.pg_pool() {
            return billing_pg::event_seen(pool, event_id).await;
        }
        self.with_db(|connection| {
            connection
                .query_row(schema::BILLING_EVENT_SEEN, [event_id], |_| Ok(true))
                .optional()
                .map(|row| row.is_some())
        })
    }

    async fn mark_event(&self, event_id: &str, created: i64) -> Result<(), ApiError> {
        let Some(db) = self.db.get() else {
            return Err(billing_disabled());
        };
        if let Some(pool) = db.pg_pool() {
            return billing_pg::mark_event(pool, event_id, created).await;
        }
        self.with_db(|connection| {
            connection.execute(
                &schema::insert_billing_event_once(Dialect::Sqlite),
                params![event_id, created],
            )?;
            let cutoff = stripe::unix_now() - 30 * 24 * 60 * 60;
            connection.execute(schema::BILLING_EVENT_DELETE_OLD, [cutoff])?;
            Ok(())
        })
    }

    fn with_db<T>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, ApiError> {
        let Some(db) = self.db.get() else {
            return Err(billing_disabled());
        };
        let Some(sqlite) = db.sqlite_connection() else {
            return Err(billing_disabled());
        };
        let connection = lock_db(&sqlite)?;
        f(&connection).map_err(db_error_raw)
    }
}

fn trial_open(account: &Account) -> bool {
    account.status == "trialing"
        && account
            .trial_end
            .is_some_and(|end| stripe::unix_now() < end)
}

/// A failed or finished subscription stays locked even when WorkOS has not yet removed `cadri`
/// from the access token. `paused` is not in this list: a paused trial unlocks when the
/// entitlement arrives, which is the signal that Stripe has accepted payment.
fn blocks_access(status: &str) -> bool {
    matches!(
        status,
        "past_due" | "unpaid" | "canceled" | "incomplete" | "incomplete_expired"
    )
}

fn account_access(entitled: bool, account: &Account) -> bool {
    if blocks_access(&account.status) {
        return false;
    }
    entitled || trial_open(account)
}

fn customer_id(event: &Event) -> Result<String, ApiError> {
    event
        .data
        .object
        .get("customer")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "invalid_payload"))
}

/// `null` means Stripe will not try the invoice again. A missing field does not, because some
/// Stripe automation deliveries omit it on the first failure.
fn retries_exhausted(invoice: &Value) -> bool {
    matches!(invoice.get("next_payment_attempt"), Some(Value::Null))
}

fn billing_disabled() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "billing_disabled")
}

fn stripe_error(err: StripeError) -> ApiError {
    match err {
        StripeError::Unavailable => ApiError::new(StatusCode::BAD_GATEWAY, "stripe_unavailable"),
        StripeError::Api(code) => {
            eprintln!("Stripe rejected the request: {code}");
            ApiError::new(StatusCode::BAD_GATEWAY, "stripe_rejected")
        }
    }
}

fn lock_db(db: &Mutex<Connection>) -> Result<std::sync::MutexGuard<'_, Connection>, ApiError> {
    db.lock()
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "database lock poisoned"))
}

fn db_error<T>(err: rusqlite::Error) -> Result<T, ApiError> {
    Err(db_error_raw(err))
}

fn db_error_raw(err: rusqlite::Error) -> ApiError {
    db_error_message(err)
}

pub(crate) async fn record_signup(pool: &sqlx::PgPool, user_id: &str) -> Result<bool, ApiError> {
    billing_pg::note_signup(pool, user_id).await
}

pub(crate) fn db_error_message(err: impl std::fmt::Display) -> ApiError {
    eprintln!("billing database error: {err}");
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "billing_unavailable")
}

/// `Stripe-Signature` is `t=<unix>,v1=<hex hmac-sha256 of "{t}.{payload}">`.
pub fn signature_is_valid(payload: &[u8], header: &str, secret: &str, now: i64) -> bool {
    let mut timestamp = None;
    let mut signatures = Vec::new();
    for part in header.split(',') {
        let Some((key, value)) = part.trim().split_once('=') else {
            continue;
        };
        match key {
            "t" => timestamp = value.parse::<i64>().ok(),
            "v1" => signatures.push(value),
            _ => {}
        }
    }
    let Some(timestamp) = timestamp else {
        return false;
    };
    if now.abs_diff(timestamp) > 300 {
        return false;
    }
    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(payload);
    let expected = mac.finalize().into_bytes();
    signatures.into_iter().any(|signature| {
        let Some(bytes) = decode_hex(signature) else {
            return false;
        };
        bytes.len() == expected.len() && bool::from(bytes.as_slice().ct_eq(expected.as_slice()))
    })
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
        .collect()
}

pub fn sign_webhook(secret: &str, timestamp: i64, payload: &[u8]) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac accepts any key length");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(payload);
    let hex = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("t={timestamp},v1={hex}")
}

/// Billing rows by status. A missing table leaves the gauges untouched.
pub(crate) async fn refresh_org_gauges(db: &cadri_core::Database) {
    if let Some(pool) = db.pg_pool() {
        let Ok(counts) = billing_pg::status_counts(pool).await else {
            return;
        };
        publish_status_counts(counts);
        return;
    }
    let Some(sqlite) = db.sqlite_connection() else {
        return;
    };
    let Ok(connection) = sqlite.lock() else {
        return;
    };
    let Ok(mut statement) = connection.prepare(schema::BILLING_STATUS_COUNTS) else {
        return;
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    }) else {
        return;
    };
    let counts: Vec<(String, i64)> = rows.flatten().collect();
    drop(statement);
    drop(connection);
    publish_status_counts(counts);
}

fn publish_status_counts(counts: Vec<(String, i64)>) {
    cadri_core::reset_org_statuses();
    let mut other = 0_i64;
    for (status, count) in counts {
        match status.as_str() {
            "trialing" | "active" | "canceled" | "past_due" | "unpaid" | "paused"
            | "incomplete" | "incomplete_expired" => {
                cadri_core::set_org_status(&status, count);
            }
            _ => other += count,
        }
    }
    if other > 0 {
        cadri_core::set_org_status("other", other);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stripe_signature_accepts_the_payload_it_signed() {
        let payload = br#"{"id":"evt_1"}"#;
        let header = sign_webhook("whsec_test", 1_700_000_000, payload);
        assert!(signature_is_valid(
            payload,
            &header,
            "whsec_test",
            1_700_000_100
        ));
        assert!(!signature_is_valid(
            payload,
            &header,
            "whsec_other",
            1_700_000_100
        ));
        assert!(!signature_is_valid(
            payload,
            &header,
            "whsec_test",
            1_700_000_400
        ));
        assert!(!signature_is_valid(
            b"tampered",
            &header,
            "whsec_test",
            1_700_000_100
        ));
    }
}
