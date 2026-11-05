//! In-process S3 stand-in for race and crash tests.

use super::etag;

/// What the next `refs/heads/main` PUT should do. Object PUTs are unaffected,
/// which is the crash window between the bundle write and the ref swap.
pub enum RefFault {
    /// The ref is left unchanged and the caller sees a failed PUT.
    Refuse,
    /// The ref is stored, then the caller sees a failed PUT.
    StoreThenFail,
}

struct FakeState {
    objects: std::collections::HashMap<String, Vec<u8>>,
    fault: Option<RefFault>,
    /// The next object PUT fails and stores nothing.
    object_fault: bool,
    /// `If-Match` / `If-None-Match` rejections of the ref only.
    ref_conflicts: usize,
    /// The next conditional PUT returns 409 and stores nothing.
    conflicts_left: u32,
    /// The `If-Match` value from the latest ref update, when the client sent one.
    last_ref_match: Option<String>,
    saw_checksum: bool,
    latency_ms: u64,
    gets: usize,
    puts: usize,
}

/// Local S3 stand-in. It ignores signatures and speaks the two calls the
/// publisher uses: GET, and PUT with `If-None-Match` / `If-Match`.
///
/// `aws_smithy_mocks` replays a scripted status for each call. A race needs the
/// second PUT's result to depend on the bytes the first PUT stored, so this
/// stand-in keeps the objects.
pub struct FakeS3 {
    pub endpoint: String,
    state: std::sync::Arc<std::sync::Mutex<FakeState>>,
}

impl FakeS3 {
    pub fn start() -> Self {
        use std::sync::{Arc, Mutex};

        let state = Arc::new(Mutex::new(FakeState {
            objects: std::collections::HashMap::new(),
            fault: None,
            object_fault: false,
            ref_conflicts: 0,
            conflicts_left: 0,
            last_ref_match: None,
            saw_checksum: false,
            latency_ms: 0,
            gets: 0,
            puts: 0,
        }));
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let shared = state.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = listener.local_addr().unwrap();
                tx.send(addr).unwrap();
                let app = axum::Router::new().fallback(fake_route).with_state(shared);
                axum::serve(listener, app).await.unwrap();
            });
        });
        let addr = rx.recv().unwrap();
        Self {
            endpoint: format!("http://{addr}"),
            state,
        }
    }

    pub fn remove(&self, key: &str) {
        self.state.lock().unwrap().objects.remove(key);
    }

    pub fn fail_next_ref_put(&self, fault: RefFault) {
        self.state.lock().unwrap().fault = Some(fault);
    }

    pub fn fail_next_object_put(&self) {
        self.state.lock().unwrap().object_fault = true;
    }

    pub fn conflict_once(&self) {
        self.state.lock().unwrap().conflicts_left = 1;
    }

    pub fn last_ref_match(&self) -> Option<String> {
        self.state.lock().unwrap().last_ref_match.clone()
    }

    pub fn saw_checksum(&self) -> bool {
        self.state.lock().unwrap().saw_checksum
    }

    pub fn ref_conflicts(&self) -> usize {
        self.state.lock().unwrap().ref_conflicts
    }

    /// Delays every request by `per_request` before it is handled.
    pub fn set_latency(&self, per_request: std::time::Duration) {
        self.state.lock().unwrap().latency_ms = per_request.as_millis() as u64;
    }

    /// Requests received so far, as `(gets, puts)`.
    pub fn requests(&self) -> (usize, usize) {
        let state = self.state.lock().unwrap();
        (state.gets, state.puts)
    }

    pub fn keys(&self) -> Vec<String> {
        self.state.lock().unwrap().objects.keys().cloned().collect()
    }
}

async fn fake_route(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<std::sync::Mutex<FakeState>>>,
    request: axum::extract::Request,
) -> axum::response::Response {
    use axum::body::Body;
    use axum::http::{header, StatusCode};
    use axum::response::Response;

    let path = request.uri().path().trim_start_matches('/');
    let Some((_, key)) = path.split_once('/') else {
        return xml(StatusCode::NOT_FOUND, "NoSuchKey");
    };
    let key = key.to_string();
    let latency = {
        let mut state = state.lock().unwrap();
        match request.method().as_str() {
            "GET" => state.gets += 1,
            "PUT" => state.puts += 1,
            _ => {}
        }
        state.latency_ms
    };
    if latency > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(latency)).await;
    }
    match request.method().as_str() {
        "GET" => {
            let found = state.lock().unwrap().objects.get(&key).cloned();
            match found {
                Some(bytes) => Response::builder()
                    .status(StatusCode::OK)
                    .header(header::ETAG, etag(&bytes))
                    .body(Body::from(bytes))
                    .unwrap(),
                None => xml(StatusCode::NOT_FOUND, "NoSuchKey"),
            }
        }
        "PUT" => {
            let if_match = header_str(request.headers(), "if-match");
            let if_none = header_str(request.headers(), "if-none-match");
            let checksum = request.headers().keys().any(|name| {
                let name = name.as_str();
                name.starts_with("x-amz-checksum-")
                    || name == "x-amz-sdk-checksum-algorithm"
                    || name == "x-amz-trailer"
            }) || request
                .headers()
                .get("content-encoding")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.contains("aws-chunked"));
            let bytes = axum::body::to_bytes(request.into_body(), 64 * 1024 * 1024)
                .await
                .unwrap()
                .to_vec();
            let mut state = state.lock().unwrap();
            let current = state.objects.get(&key).cloned();
            let is_ref = key.ends_with("refs/heads/main");
            if is_ref {
                if if_match.is_some() {
                    state.last_ref_match = if_match.clone();
                }
                if checksum {
                    state.saw_checksum = true;
                }
            }
            if !is_ref && state.object_fault {
                state.object_fault = false;
                return xml(StatusCode::INTERNAL_SERVER_ERROR, "InternalError");
            }
            if state.conflicts_left > 0 {
                state.conflicts_left -= 1;
                return xml(StatusCode::CONFLICT, "ConditionalRequestConflict");
            }
            let reject = match &if_match {
                Some(tag) => !matches!(&current, Some(existing) if etag_matches(tag, existing)),
                None => false,
            } || (if_none.as_deref().is_some_and(|value| value.trim() == "*")
                && current.is_some());
            if reject {
                if is_ref {
                    state.ref_conflicts += 1;
                }
                return xml(StatusCode::PRECONDITION_FAILED, "PreconditionFailed");
            }
            let fault = if is_ref { state.fault.take() } else { None };
            if matches!(fault, Some(RefFault::Refuse)) {
                return xml(StatusCode::INTERNAL_SERVER_ERROR, "InternalError");
            }
            let tag = etag(&bytes);
            state.objects.insert(key, bytes);
            if matches!(fault, Some(RefFault::StoreThenFail)) {
                return xml(StatusCode::INTERNAL_SERVER_ERROR, "InternalError");
            }
            Response::builder()
                .status(StatusCode::OK)
                .header(header::ETAG, tag)
                .body(Body::empty())
                .unwrap()
        }
        _ => xml(StatusCode::METHOD_NOT_ALLOWED, "MethodNotAllowed"),
    }
}

fn header_str(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

fn etag_matches(header: &str, body: &[u8]) -> bool {
    // Real S3 compares the quoted ETag. An unquoted hex digest must not match,
    // or this stand-in would hide a client that forgot the quotes.
    let want = etag(body);
    header.split(',').any(|part| part.trim() == want)
}

fn xml(status: axum::http::StatusCode, code: &str) -> axum::response::Response {
    use axum::body::Body;
    use axum::http::header;
    use axum::response::Response;

    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Error><Code>{code}</Code><Message>{code}</Message></Error>"#
    );
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/xml")
        .body(Body::from(body))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_latency_delays_each_request() {
        let fake = FakeS3::start();
        fake.set_latency(std::time::Duration::from_millis(50));
        let started = std::time::Instant::now();
        let mut stream =
            std::net::TcpStream::connect(fake.endpoint.trim_start_matches("http://")).unwrap();
        use std::io::{Read, Write};
        stream
            .write_all(b"GET /bucket/missing HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        assert!(started.elapsed() >= std::time::Duration::from_millis(50));
        assert_eq!(fake.requests(), (1, 0));
    }
}
