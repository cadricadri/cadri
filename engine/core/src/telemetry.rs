//! Prometheus metrics for HTTP requests and browser page loads.
//!
//! `/metrics` is mounted on the engine listener. The public nginx config does not
//! proxy it, so a cluster scrape of Service `engine` is the intended access path.
//! `POST /api/telemetry/page` is public because `sendBeacon` cannot set an
//! `Authorization` header. The handler keeps only an allowlisted page label.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use axum::extract::Request;
use axum::http::{header, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use prometheus::Encoder;
use prometheus::{
    Histogram, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge, IntGaugeVec, Opts,
    Registry, TextEncoder, TEXT_FORMAT,
};
use rusqlite::Connection;
use serde::Deserialize;

const HTTP_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 120.0,
];

/// Page-load buckets go well past a minute because the memories screen can sit
/// on a slow document load. The git fetch itself is the `/api/memories` histogram.
const PAGE_BUCKETS: &[f64] = &[
    0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0,
];

const MAX_PAGE_SECONDS: f64 = 24.0 * 60.0 * 60.0;
const MAX_PAGE_BODY: usize = 1024;

/// Byte sizes from a small note through the 32 MiB request cap.
const BYTE_BUCKETS: &[f64] = &[
    256.0,
    1024.0,
    4096.0,
    16384.0,
    65536.0,
    262144.0,
    1_048_576.0,
    4_194_304.0,
    16_777_216.0,
    33_554_432.0,
];

const ORG_STATUSES: &[&str] = &[
    "trialing",
    "active",
    "canceled",
    "past_due",
    "unpaid",
    "paused",
    "incomplete",
    "incomplete_expired",
    "other",
];

const CONTRIBUTOR_BUCKETS: &[&str] = &["1", "2-3", "4-8", "9+"];

const PLUGIN_COMMANDS: &[&str] = &[
    "init",
    "login",
    "uninstall",
    "discover",
    "show",
    "memory_push",
    "memory_sync",
    "memory_search",
    "memory_context",
    "memory_status",
    "memory_clone",
    "memory_path",
    "memory_write",
    "hook_session-start",
    "hook_prompt",
    "hook_tool-after",
    "hook_edit",
    "hook_response",
    "hook_stop",
    "hook_pre-compact",
    "mcp_cadri_search",
];

struct Metrics {
    registry: Registry,
    http_duration: HistogramVec,
    http_requests: IntCounterVec,
    http_in_flight: IntGauge,
    page_load: HistogramVec,
    page_content: HistogramVec,
    client_duration: HistogramVec,
    plugin_duration: HistogramVec,
    git_duration: HistogramVec,
    git_children: IntGauge,
    s3_put: Histogram,
    signups: IntCounter,
    sessions_uploaded: IntCounter,
    session_bytes: Histogram,
    memory_commits: IntCounterVec,
    commit_bytes: Histogram,
    billing_transitions: IntCounterVec,
    orgs: IntGaugeVec,
    orgs_by_contributors: IntGaugeVec,
}

type ScrapeHook = Arc<dyn Fn() + Send + Sync>;

static DB: Mutex<Option<Arc<Mutex<Connection>>>> = Mutex::new(None);
static SCRAPE_HOOK: Mutex<Option<ScrapeHook>> = Mutex::new(None);

fn metrics() -> &'static Metrics {
    static METRICS: OnceLock<Metrics> = OnceLock::new();
    METRICS.get_or_init(|| {
        let registry = Registry::new();
        let http_duration = HistogramVec::new(
            HistogramOpts::new(
                "cadri_http_request_duration_seconds",
                "HTTP request duration in seconds.",
            )
            .buckets(HTTP_BUCKETS.to_vec()),
            &["method", "route", "status_class"],
        )
        .expect("http duration histogram");
        let http_requests = IntCounterVec::new(
            Opts::new("cadri_http_requests_total", "HTTP requests."),
            &["method", "route", "status_class"],
        )
        .expect("http request counter");
        let page_load = histogram(
            "cadri_page_load_duration_seconds",
            "Browser document load and in-app route-change duration in seconds.",
            PAGE_BUCKETS,
            &["page"],
        );
        let page_content = histogram(
            "cadri_page_content_duration_seconds",
            "Navigation start until the page has painted server data, in seconds.",
            PAGE_BUCKETS,
            &["page"],
        );
        let client_duration = histogram(
            "cadri_client_request_duration_seconds",
            "Browser fetch duration in seconds, including network and nginx.",
            HTTP_BUCKETS,
            &["route"],
        );
        let plugin_duration = histogram(
            "cadri_plugin_duration_seconds",
            "Plugin CLI, hook, or MCP call duration in seconds.",
            HTTP_BUCKETS,
            &["command"],
        );
        let git_duration = histogram(
            "cadri_git_duration_seconds",
            "Engine git subprocess duration in seconds.",
            HTTP_BUCKETS,
            &["op"],
        );
        let git_children = IntGauge::new("cadri_git_children", "Git child processes running now.")
            .expect("git children");
        let s3_put = bare_histogram(
            "cadri_s3_put_duration_seconds",
            "Memory bundle PUT duration in seconds.",
            HTTP_BUCKETS,
        );
        let http_in_flight = IntGauge::new(
            "cadri_http_in_flight",
            "HTTP requests currently inside the engine.",
        )
        .expect("in flight");
        let signups = IntCounter::new("cadri_signups_total", "First WorkOS login of a user.")
            .expect("signups");
        let sessions_uploaded = IntCounter::new(
            "cadri_sessions_uploaded_total",
            "Session rows inserted. Updates of an existing session are not counted.",
        )
        .expect("sessions");
        let session_bytes = bare_histogram(
            "cadri_session_upload_bytes",
            "Bytes stored for a newly inserted session after keeping the chat, plans, and tool names.",
            BYTE_BUCKETS,
        );
        let memory_commits = IntCounterVec::new(
            Opts::new(
                "cadri_memory_commits_total",
                "Accepted memory commits. origin is seed or steady.",
            ),
            &["origin"],
        )
        .expect("commits");
        let commit_bytes = bare_histogram(
            "cadri_commit_bytes",
            "New blob bytes in an accepted memory commit.",
            BYTE_BUCKETS,
        );
        let billing_transitions = IntCounterVec::new(
            Opts::new(
                "cadri_billing_transitions_total",
                "Billing status changes. from and to are allowlisted statuses.",
            ),
            &["from", "to"],
        )
        .expect("billing transitions");
        let orgs = IntGaugeVec::new(
            Opts::new("cadri_orgs", "Billing rows by status."),
            &["status"],
        )
        .expect("orgs");
        let orgs_by_contributors = IntGaugeVec::new(
            Opts::new(
                "cadri_orgs_by_contributors",
                "Orgs by how many distinct users have uploaded a session.",
            ),
            &["bucket"],
        )
        .expect("contributors");
        for metric in [
            Box::new(http_duration.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(http_requests.clone()),
            Box::new(http_in_flight.clone()),
            Box::new(page_load.clone()),
            Box::new(page_content.clone()),
            Box::new(client_duration.clone()),
            Box::new(plugin_duration.clone()),
            Box::new(git_duration.clone()),
            Box::new(git_children.clone()),
            Box::new(s3_put.clone()),
            Box::new(signups.clone()),
            Box::new(sessions_uploaded.clone()),
            Box::new(session_bytes.clone()),
            Box::new(memory_commits.clone()),
            Box::new(commit_bytes.clone()),
            Box::new(billing_transitions.clone()),
            Box::new(orgs.clone()),
            Box::new(orgs_by_contributors.clone()),
        ] {
            registry.register(metric).expect("register metric");
        }
        Metrics {
            registry,
            http_duration,
            http_requests,
            http_in_flight,
            page_load,
            page_content,
            client_duration,
            plugin_duration,
            git_duration,
            git_children,
            s3_put,
            signups,
            sessions_uploaded,
            session_bytes,
            memory_commits,
            commit_bytes,
            billing_transitions,
            orgs,
            orgs_by_contributors,
        }
    })
}

fn histogram(name: &str, help: &str, buckets: &[f64], labels: &[&str]) -> HistogramVec {
    HistogramVec::new(
        HistogramOpts::new(name, help).buckets(buckets.to_vec()),
        labels,
    )
    .unwrap_or_else(|_| panic!("histogram {name}"))
}

fn bare_histogram(name: &str, help: &str, buckets: &[f64]) -> Histogram {
    Histogram::with_opts(HistogramOpts::new(name, help).buckets(buckets.to_vec()))
        .unwrap_or_else(|_| panic!("histogram {name}"))
}

pub fn routes() -> Router {
    Router::new()
        .route("/metrics", get(scrape))
        .route("/api/telemetry/page", post(page_load))
        .route("/api/telemetry/client", post(client_timing))
        .route("/api/telemetry/content", post(content_timing))
        .route("/api/telemetry/plugin", post(plugin_timing))
}

/// Latest engine database. Contributor gauges are counted from it at scrape time.
pub(crate) fn bind_db(db: Arc<Mutex<Connection>>) {
    if let Ok(mut slot) = DB.lock() {
        *slot = Some(db);
    }
}

/// Replaces the hosted-only scrape hook (billing row counts). Core does not call WorkOS.
pub fn set_scrape_hook(hook: impl Fn() + Send + Sync + 'static) {
    if let Ok(mut slot) = SCRAPE_HOOK.lock() {
        *slot = Some(Arc::new(hook));
    }
}

pub async fn track(request: Request, next: Next) -> Response {
    let method = method_label(request.method());
    let route = route_template(request.uri().path());
    let started = Instant::now();
    // Probes would otherwise pin this gauge at 1: the scrape reads it while
    // the scrape request is still inside this middleware.
    let _in_flight = InFlight::enter(route != "/metrics" && route != "/healthz");
    let response = next.run(request).await;
    let class = status_class(response.status());
    let metrics = metrics();
    metrics
        .http_duration
        .with_label_values(&[method, route, class])
        .observe(started.elapsed().as_secs_f64());
    metrics
        .http_requests
        .with_label_values(&[method, route, class])
        .inc();
    response
}

struct InFlight(bool);

impl InFlight {
    fn enter(count: bool) -> Self {
        if count {
            metrics().http_in_flight.inc();
        }
        InFlight(count)
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if self.0 {
            metrics().http_in_flight.dec();
        }
    }
}

/// One inserted session. `bytes` is the stored body length.
pub(crate) fn observe_session(bytes: u64) {
    let metrics = metrics();
    metrics.sessions_uploaded.inc();
    metrics.session_bytes.observe(bytes as f64);
}

/// One accepted memory commit. `origin` is `seed` or anything else (`steady`).
pub(crate) fn observe_commit(origin: &str, bytes: u64) {
    let origin = if origin == "seed" { "seed" } else { "steady" };
    let metrics = metrics();
    metrics.memory_commits.with_label_values(&[origin]).inc();
    metrics.commit_bytes.observe(bytes as f64);
}

/// First time this process records that user. The caller decides what "first" means.
pub fn observe_signup() {
    metrics().signups.inc();
}

pub fn observe_billing_transition(from: &str, to: &str) {
    let from = billing_status(from);
    let to = billing_status(to);
    if from == to {
        return;
    }
    metrics()
        .billing_transitions
        .with_label_values(&[from, to])
        .inc();
}

pub fn reset_org_statuses() {
    let metrics = metrics();
    for status in ORG_STATUSES {
        metrics.orgs.with_label_values(&[status]).set(0);
    }
}

pub fn set_org_status(status: &str, count: i64) {
    metrics()
        .orgs
        .with_label_values(&[billing_status(status)])
        .set(count);
}

pub(crate) fn observe_git(args: &[&str], seconds: f64) {
    if !seconds.is_finite() || seconds < 0.0 {
        return;
    }
    metrics()
        .git_duration
        .with_label_values(&[git_op(args)])
        .observe(seconds);
}

pub(crate) struct GitChild;

impl GitChild {
    pub(crate) fn enter() -> Self {
        metrics().git_children.inc();
        GitChild
    }
}

impl Drop for GitChild {
    fn drop(&mut self) {
        metrics().git_children.dec();
    }
}

pub(crate) fn observe_s3_put(seconds: f64) {
    if !seconds.is_finite() || seconds < 0.0 {
        return;
    }
    metrics().s3_put.observe(seconds);
}

fn git_op(args: &[&str]) -> &'static str {
    let mut args = args.iter().copied();
    let name = loop {
        match args.next() {
            Some("-c") => {
                let _ = args.next();
            }
            Some("--") => continue,
            Some(arg) if arg.starts_with('-') => continue,
            Some(arg) => break arg,
            None => break "other",
        }
    };
    match name {
        "blame" => "blame",
        "log" => "log",
        "index-pack" => "index-pack",
        "upload-pack" => "upload-pack",
        "bundle" => "bundle",
        "commit-tree" => "commit-tree",
        "rev-list" => "rev-list",
        "cat-file" => "cat-file",
        "diff-tree" => "diff-tree",
        "hash-object" => "hash-object",
        "mktree" => "mktree",
        "update-ref" => "update-ref",
        "init" => "init",
        "rev-parse" => "rev-parse",
        "merge-base" => "merge-base",
        "config" => "config",
        _ => "other",
    }
}

fn billing_status(status: &str) -> &'static str {
    match status {
        "none" => "none",
        "trialing" => "trialing",
        "active" => "active",
        "canceled" => "canceled",
        "past_due" => "past_due",
        "unpaid" => "unpaid",
        "paused" => "paused",
        "incomplete" => "incomplete",
        "incomplete_expired" => "incomplete_expired",
        _ => "other",
    }
}

async fn scrape() -> impl IntoResponse {
    refresh_contributors();
    if let Some(hook) = SCRAPE_HOOK.lock().ok().and_then(|slot| slot.clone()) {
        hook();
    }
    let encoder = TextEncoder::new();
    let mut buffer = Vec::new();
    encoder
        .encode(&metrics().registry.gather(), &mut buffer)
        .expect("encode prometheus metrics");
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, TEXT_FORMAT)],
        buffer,
    )
}

fn refresh_contributors() {
    let Some(db) = DB.lock().ok().and_then(|slot| slot.clone()) else {
        return;
    };
    let Ok(connection) = db.lock() else {
        return;
    };
    let metrics = metrics();
    for bucket in CONTRIBUTOR_BUCKETS {
        metrics
            .orgs_by_contributors
            .with_label_values(&[bucket])
            .set(0);
    }
    let Ok(mut statement) = connection.prepare(
        "SELECT COUNT(DISTINCT user_id) FROM sessions
         WHERE IFNULL(user_id, '') != ''
         GROUP BY org_id",
    ) else {
        return;
    };
    let Ok(rows) = statement.query_map([], |row| row.get::<_, i64>(0)) else {
        return;
    };
    let mut counts = [0_i64; 4];
    for count in rows.flatten() {
        let index = match count {
            1 => 0,
            2 | 3 => 1,
            4..=8 => 2,
            9.. => 3,
            _ => continue,
        };
        counts[index] += 1;
    }
    for (bucket, count) in CONTRIBUTOR_BUCKETS.iter().zip(counts) {
        metrics
            .orgs_by_contributors
            .with_label_values(&[bucket])
            .set(count);
    }
}

#[derive(Deserialize)]
struct PageLoad {
    page: String,
    duration_ms: f64,
}

async fn page_load(request: Request) -> StatusCode {
    let bytes = match axum::body::to_bytes(request.into_body(), MAX_PAGE_BODY).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE,
    };
    let Ok(body) = serde_json::from_slice::<PageLoad>(&bytes) else {
        return StatusCode::BAD_REQUEST;
    };
    if !observe_page_load(&body.page, body.duration_ms) {
        return StatusCode::BAD_REQUEST;
    }
    StatusCode::NO_CONTENT
}

fn observe_page_load(page: &str, duration_ms: f64) -> bool {
    observe_page_histogram(&metrics().page_load, page, duration_ms)
}

#[derive(Deserialize)]
struct ClientTiming {
    path: String,
    duration_ms: f64,
}

async fn client_timing(request: Request) -> StatusCode {
    let bytes = match axum::body::to_bytes(request.into_body(), MAX_PAGE_BODY).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE,
    };
    let Ok(body) = serde_json::from_slice::<ClientTiming>(&bytes) else {
        return StatusCode::BAD_REQUEST;
    };
    let Some(seconds) = duration_seconds(body.duration_ms) else {
        return StatusCode::BAD_REQUEST;
    };
    let Some(route) = client_route(&body.path) else {
        return StatusCode::BAD_REQUEST;
    };
    metrics()
        .client_duration
        .with_label_values(&[route])
        .observe(seconds);
    StatusCode::NO_CONTENT
}

async fn content_timing(request: Request) -> StatusCode {
    let bytes = match axum::body::to_bytes(request.into_body(), MAX_PAGE_BODY).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE,
    };
    let Ok(body) = serde_json::from_slice::<PageLoad>(&bytes) else {
        return StatusCode::BAD_REQUEST;
    };
    if !observe_page_histogram(&metrics().page_content, &body.page, body.duration_ms) {
        return StatusCode::BAD_REQUEST;
    }
    StatusCode::NO_CONTENT
}

#[derive(Deserialize)]
struct PluginTiming {
    command: String,
    duration_ms: f64,
}

async fn plugin_timing(request: Request) -> StatusCode {
    let bytes = match axum::body::to_bytes(request.into_body(), MAX_PAGE_BODY).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE,
    };
    let Ok(body) = serde_json::from_slice::<PluginTiming>(&bytes) else {
        return StatusCode::BAD_REQUEST;
    };
    let Some(seconds) = duration_seconds(body.duration_ms) else {
        return StatusCode::BAD_REQUEST;
    };
    let Some(command) = plugin_command(&body.command) else {
        return StatusCode::BAD_REQUEST;
    };
    metrics()
        .plugin_duration
        .with_label_values(&[command])
        .observe(seconds);
    StatusCode::NO_CONTENT
}

fn observe_page_histogram(histogram: &HistogramVec, page: &str, duration_ms: f64) -> bool {
    let Some(seconds) = duration_seconds(duration_ms) else {
        return false;
    };
    histogram
        .with_label_values(&[page_label(page)])
        .observe(seconds);
    true
}

fn duration_seconds(duration_ms: f64) -> Option<f64> {
    if !duration_ms.is_finite() || duration_ms < 0.0 {
        return None;
    }
    Some((duration_ms / 1000.0).min(MAX_PAGE_SECONDS))
}

/// Browser fetch path. Telemetry posts and probes are not client-latency series.
fn client_route(path: &str) -> Option<&'static str> {
    let path = path.split('?').next().unwrap_or(path);
    match route_template(path) {
        "unmatched" | "/healthz" | "/metrics" => None,
        route if route.starts_with("/api/telemetry/") => None,
        route => Some(route),
    }
}

fn plugin_command(command: &str) -> Option<&'static str> {
    PLUGIN_COMMANDS
        .iter()
        .copied()
        .find(|allowed| *allowed == command)
}

fn method_label(method: &Method) -> &'static str {
    if method == Method::GET {
        "GET"
    } else if method == Method::POST {
        "POST"
    } else if method == Method::PUT {
        "PUT"
    } else if method == Method::DELETE {
        "DELETE"
    } else if method == Method::PATCH {
        "PATCH"
    } else if method == Method::HEAD {
        "HEAD"
    } else if method == Method::OPTIONS {
        "OPTIONS"
    } else {
        "OTHER"
    }
}

fn status_class(status: StatusCode) -> &'static str {
    match status.as_u16() {
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        500..=599 => "5xx",
        _ => "other",
    }
}

/// Stable route template. New engine routes must be added here. Anything else
/// shares the single `unmatched` label so a raw id, email, or repo name cannot
/// become a series.
fn route_template(path: &str) -> &'static str {
    match path {
        "/healthz" => "/healthz",
        "/metrics" => "/metrics",
        "/api/auth/config" => "/api/auth/config",
        "/api/me" => "/api/me",
        "/api/sessions" => "/api/sessions",
        "/api/transcripts" => "/api/sessions",
        "/api/memories" => "/api/memories",
        "/api/memories/tree" => "/api/memories/tree",
        "/api/memories/tip" => "/api/memories/tip",
        "/api/memories/log" => "/api/memories/log",
        "/api/memories/blame" => "/api/memories/blame",
        "/api/memories/commits" => "/api/memories/commits",
        "/api/telemetry/page" => "/api/telemetry/page",
        "/api/telemetry/client" => "/api/telemetry/client",
        "/api/telemetry/content" => "/api/telemetry/content",
        "/api/telemetry/plugin" => "/api/telemetry/plugin",
        "/api/billing" => "/api/billing",
        "/api/billing/portal" => "/api/billing/portal",
        "/api/stripe/webhook" => "/api/stripe/webhook",
        "/auth/login" => "/auth/login",
        "/auth/callback" => "/auth/callback",
        "/auth/logout" => "/auth/logout",
        "/auth/organization" => "/auth/organization",
        "/auth/invitations" => "/auth/invitations",
        "/auth/team" => "/auth/team",
        "/api/billing/invoices" => "/api/billing/invoices",
        "/auth/device/start" => "/auth/device/start",
        "/auth/device/token" => "/auth/device/token",
        "/auth/refresh" => "/auth/refresh",
        _ if is_session_route(path) => "/api/sessions/{session_id}",
        _ if path.starts_with("/api/memories/blob/") => "/api/memories/blob/{sha}",
        _ if is_git_route(path) => "/git/{book}/{path}",
        _ => auth_id_route(path).unwrap_or("unmatched"),
    }
}

/// Routes whose only variable segment is an object id. The id is not a label.
fn auth_id_route(path: &str) -> Option<&'static str> {
    if let Some(rest) = path.strip_prefix("/auth/invitations/") {
        let mut parts = rest.split('/');
        let id = parts.next().unwrap_or("");
        let action = parts.next();
        if id.is_empty() || parts.next().is_some() {
            return None;
        }
        return match action {
            Some("revoke") => Some("/auth/invitations/{id}/revoke"),
            Some("resend") => Some("/auth/invitations/{id}/resend"),
            _ => None,
        };
    }
    if let Some(id) = path.strip_prefix("/auth/memberships/") {
        if !id.is_empty() && !id.contains('/') {
            return Some("/auth/memberships/{id}");
        }
    }
    None
}

fn is_session_route(path: &str) -> bool {
    for prefix in ["/api/sessions/", "/api/transcripts/"] {
        if let Some(id) = path.strip_prefix(prefix) {
            return !id.is_empty() && !id.contains('/');
        }
    }
    false
}

fn is_git_route(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/git/") else {
        return false;
    };
    let Some((book, remainder)) = rest.split_once('/') else {
        return false;
    };
    !book.is_empty() && !remainder.is_empty()
}

/// SPA routes. Memories is `/`. Unknown paths, including ones that contain ids
/// or emails, collapse to `other`.
fn page_label(page: &str) -> &'static str {
    match page {
        "/" => "/",
        "/sessions" | "/transcripts" => "/sessions",
        "/usage" => "/usage",
        "/team" => "/team",
        "/billing" => "/billing",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_keep_labels_low_cardinality() {
        assert_eq!(route_template("/api/sessions"), "/api/sessions");
        assert_eq!(route_template("/api/transcripts"), "/api/sessions");
        assert_eq!(
            route_template("/api/sessions/sess-1"),
            "/api/sessions/{session_id}"
        );
        assert_eq!(
            route_template("/api/transcripts/sess-1"),
            "/api/sessions/{session_id}"
        );
        assert_eq!(route_template("/api/memories"), "/api/memories");
        assert_eq!(route_template("/api/memories/log"), "/api/memories/log");
        assert_eq!(route_template("/api/memories/tree"), "/api/memories/tree");
        assert_eq!(route_template("/api/memories/tip"), "/api/memories/tip");
        assert_eq!(
            route_template("/api/memories/blob/0123abcd"),
            "/api/memories/blob/{sha}"
        );
        assert_eq!(route_template("/api/memories/blame"), "/api/memories/blame");
        assert_eq!(
            route_template("/git/team.git/info/refs"),
            "/git/{book}/{path}"
        );
        assert_eq!(
            route_template("/git/personal.git/objects/aa/bb"),
            "/git/{book}/{path}"
        );
        assert_eq!(
            route_template("/git/user.git/objects/aa/bb"),
            "/git/{book}/{path}"
        );
        assert_eq!(route_template("/nope/repo-name"), "unmatched");
        assert_eq!(
            route_template("/auth/invitations/inv_123/revoke"),
            "/auth/invitations/{id}/revoke"
        );
        assert_eq!(
            route_template("/auth/memberships/om_123"),
            "/auth/memberships/{id}"
        );
        assert_eq!(page_label("/"), "/");
        assert_eq!(page_label("/sessions"), "/sessions");
        assert_eq!(page_label("/transcripts"), "/sessions");
        assert_eq!(page_label("user@example.com"), "other");
        assert_eq!(page_label("/team/org_abc"), "other");
        assert_eq!(status_class(StatusCode::OK), "2xx");
        assert_eq!(status_class(StatusCode::NOT_FOUND), "4xx");
        assert_eq!(status_class(StatusCode::BAD_GATEWAY), "5xx");
        assert_eq!(
            client_route("/api/memories?scope=team"),
            Some("/api/memories")
        );
        assert_eq!(client_route("/api/telemetry/page"), None);
        assert_eq!(
            client_route("/api/sessions/secret-id"),
            Some("/api/sessions/{session_id}")
        );
        assert_eq!(plugin_command("memory_push"), Some("memory_push"));
        assert_eq!(plugin_command("memory_push;rm"), None);
        assert_eq!(
            git_op(&["-c", "uploadpack.hideRefs=refs/cadri", "upload-pack"]),
            "upload-pack"
        );
        assert_eq!(
            git_op(&["rev-parse", "--verify", "refs/heads/main"]),
            "rev-parse"
        );
        assert_eq!(billing_status("past_due"), "past_due");
        assert_eq!(billing_status("nope"), "other");
    }
}
