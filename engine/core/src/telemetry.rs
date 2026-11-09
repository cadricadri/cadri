//! Prometheus metrics for HTTP requests and browser page loads.
//!
//! `/metrics` is mounted on the engine listener. The public nginx config does not
//! proxy it, so a cluster scrape of Service `engine` is the intended access path.
//! `POST /api/telemetry/page` is public because `sendBeacon` cannot set an
//! `Authorization` header. The handler keeps only an allowlisted page label.

use std::sync::OnceLock;
use std::time::Instant;

use axum::extract::Request;
use axum::http::{header, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use prometheus::Encoder;
use prometheus::{
    HistogramOpts, HistogramVec, IntCounterVec, Opts, Registry, TextEncoder, TEXT_FORMAT,
};
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

struct Metrics {
    registry: Registry,
    http_duration: HistogramVec,
    http_requests: IntCounterVec,
    page_load: HistogramVec,
}

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
        let page_load = HistogramVec::new(
            HistogramOpts::new(
                "cadri_page_load_duration_seconds",
                "Browser document load and in-app route-change duration in seconds.",
            )
            .buckets(PAGE_BUCKETS.to_vec()),
            &["page"],
        )
        .expect("page load histogram");
        registry
            .register(Box::new(http_duration.clone()))
            .expect("register http duration");
        registry
            .register(Box::new(http_requests.clone()))
            .expect("register http requests");
        registry
            .register(Box::new(page_load.clone()))
            .expect("register page load");
        Metrics {
            registry,
            http_duration,
            http_requests,
            page_load,
        }
    })
}

pub fn routes() -> Router {
    Router::new()
        .route("/metrics", get(scrape))
        .route("/api/telemetry/page", post(page_load))
}

pub async fn track(request: Request, next: Next) -> Response {
    let method = method_label(request.method());
    let route = route_template(request.uri().path());
    let started = Instant::now();
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

async fn scrape() -> impl IntoResponse {
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
    if !duration_ms.is_finite() || duration_ms < 0.0 {
        return false;
    }
    let seconds = (duration_ms / 1000.0).min(MAX_PAGE_SECONDS);
    metrics()
        .page_load
        .with_label_values(&[page_label(page)])
        .observe(seconds);
    true
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
    }
}
