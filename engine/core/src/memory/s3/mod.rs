//! S3 publish path for a memory book.
//!
//! Keys match the hosted layout: `repos/<repo_id>/objects/<sha>` is one immutable
//! bundle, and `repos/<repo_id>/refs/heads/main` is the tip. The ref is a single
//! PUT whose body is the commit SHA plus a newline, so its ETag is the MD5 of
//! that body. Creating the ref sends `If-None-Match: *`. Moving it sends
//! `If-Match` for the previous body.
//!
//! When `CADRI_MEMORIES_ACCESS_ROLE_ARN` is set, each repo operation assumes that
//! role with a session policy limited to `repos/<repo_id>/*`. Otherwise the
//! process credentials are used (localhost, FakeS3, or until the role is wired).
//!
//! Checksums use `WhenRequired`. This SDK has no `WhenNever`. `PutObject` does
//! not require a checksum, so the body stays the bytes we send and a single-part
//! ETag stays the quoted MD5 of that body. That quoted form is what `If-Match` sends.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use aws_sdk_s3::config::{BehaviorVersion, RequestChecksumCalculation, ResponseChecksumValidation};
use aws_sdk_s3::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;
use md5::{Digest, Md5};

mod fake;

pub use fake::{FakeS3, RefFault};

/// Region string for FakeS3 / in-process test clients only.
///
/// Production always takes the region from the AWS SDK default chain
/// (`AWS_REGION` / `AWS_DEFAULT_REGION` / shared config) inside [`S3Books::from_env`].
fn test_s3_region() -> &'static str {
    "us-west-2"
}

pub struct S3Books {
    bucket: String,
    client: Client,
    access_role_arn: Option<String>,
    region: String,
    endpoint: Option<String>,
    sessions: Mutex<HashMap<String, ScopedClient>>,
    runtime: Option<tokio::runtime::Runtime>,
}

struct ScopedClient {
    client: Client,
    expires: Instant,
}

#[derive(Debug)]
pub(crate) enum S3Fail {
    Conflict,
    Message(String),
}

impl S3Books {
    /// Wrap an already-configured client. Region comes from the client config
    /// (set by [`Self::from_env`] or [`Self::test_client`]), never a second hardcode.
    pub fn from_client(bucket: impl Into<String>, client: Client) -> Self {
        let region = client
            .config()
            .region()
            .map(|region| region.to_string())
            .expect("S3 client must carry a region");
        Self::build(bucket.into(), client, None, region, None)
    }

    fn build(
        bucket: String,
        client: Client,
        access_role_arn: Option<String>,
        region: String,
        endpoint: Option<String>,
    ) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("cadri-memory-s3")
            .build()
            .expect("memory s3 runtime");
        Self {
            bucket,
            client,
            access_role_arn,
            region,
            endpoint,
            sessions: Mutex::new(HashMap::new()),
            runtime: Some(runtime),
        }
    }

    /// Client aimed at a stand-in. Checksums stay off, as on the production client,
    /// so a single PUT's ETag is the MD5 of the body.
    ///
    /// In-process FakeS3 client. Prefer [`crate::memory::testing`] from `tests/`.
    pub fn test_client(endpoint: &str) -> Client {
        use aws_sdk_s3::config::retry::RetryConfig;
        use aws_sdk_s3::config::{Credentials, Region};

        let config = aws_sdk_s3::config::Builder::new()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(test_s3_region()))
            .credentials_provider(Credentials::new("test", "test", None, None, "test"))
            .endpoint_url(endpoint)
            .force_path_style(true)
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .response_checksum_validation(ResponseChecksumValidation::WhenRequired)
            .retry_config(RetryConfig::disabled())
            .build();
        Client::from_conf(config)
    }

    pub async fn from_env(bucket: String) -> Result<Self, String> {
        let sdk = aws_config::load_defaults(BehaviorVersion::latest()).await;
        let region = sdk
            .region()
            .map(|region| region.to_string())
            .ok_or_else(|| {
                "CADRI_MEMORIES_BUCKET is set but no AWS region is configured (set AWS_REGION)"
                    .to_string()
            })?;
        let mut builder = aws_sdk_s3::config::Builder::from(&sdk)
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .response_checksum_validation(ResponseChecksumValidation::WhenRequired);
        let endpoint = std::env::var("CADRI_MEMORIES_S3_ENDPOINT")
            .ok()
            .filter(|value| !value.is_empty());
        if let Some(url) = &endpoint {
            builder = builder.endpoint_url(url).force_path_style(true);
        }
        let access_role_arn = std::env::var("CADRI_MEMORIES_ACCESS_ROLE_ARN")
            .ok()
            .filter(|value| !value.is_empty());
        Ok(Self::build(
            bucket,
            Client::from_conf(builder.build()),
            access_role_arn,
            region,
            endpoint,
        ))
    }

    pub(crate) fn read_ref(&self, repo_id: &str) -> Result<Option<String>, S3Fail> {
        let client = self.client_for(repo_id)?;
        match self.get(&client, &ref_key(repo_id))? {
            None => Ok(None),
            Some(bytes) => parse_ref(&bytes),
        }
    }

    pub(crate) fn get_object(&self, repo_id: &str, sha: &str) -> Result<Vec<u8>, S3Fail> {
        let client = self.client_for(repo_id)?;
        match self.get(&client, &obj_key(repo_id, sha))? {
            Some(bytes) => Ok(bytes),
            None => Err(S3Fail::Message(format!("memory bundle {sha} is missing"))),
        }
    }

    pub(crate) fn put_bundle(&self, repo_id: &str, sha: &str, bytes: &[u8]) -> Result<(), S3Fail> {
        let client = self.client_for(repo_id)?;
        let key = obj_key(repo_id, sha);
        let body = bytes.to_vec();
        match self.put(&client, &key, &body, PutPrecondition::Create)? {
            PutOutcome::Stored => Ok(()),
            PutOutcome::Exists => match self.get(&client, &key)? {
                Some(existing) if existing == body => Ok(()),
                Some(_) => Err(S3Fail::Message(format!(
                    "memory object {key} already holds different bytes"
                ))),
                None => Err(S3Fail::Conflict),
            },
        }
    }

    pub(crate) fn cas_ref(
        &self,
        repo_id: &str,
        expected: Option<&str>,
        new_sha: &str,
    ) -> Result<(), S3Fail> {
        let client = self.client_for(repo_id)?;
        let body = ref_body(new_sha);
        let precondition = match expected {
            None => PutPrecondition::Create,
            Some(sha) => PutPrecondition::Match(etag(&ref_body(sha))),
        };
        match self.put(&client, &ref_key(repo_id), &body, precondition)? {
            PutOutcome::Stored => Ok(()),
            PutOutcome::Exists => Err(S3Fail::Conflict),
        }
    }

    fn client_for(&self, repo_id: &str) -> Result<Client, S3Fail> {
        let Some(role_arn) = &self.access_role_arn else {
            return Ok(self.client.clone());
        };
        {
            let sessions = self.sessions.lock().expect("memory s3 sessions");
            if let Some(scoped) = sessions.get(repo_id) {
                if scoped.expires > Instant::now() + Duration::from_secs(60) {
                    return Ok(scoped.client.clone());
                }
            }
        }
        let role_arn = role_arn.clone();
        let bucket = self.bucket.clone();
        let repo = repo_id.to_string();
        let region = self.region.clone();
        let endpoint = self.endpoint.clone();
        let base = self.client.clone();
        let (client, expires) = self.drive(async move {
            assume_repo_client(base, role_arn, bucket, repo, region, endpoint).await
        })??;
        let mut sessions = self.sessions.lock().expect("memory s3 sessions");
        sessions.insert(
            repo_id.to_string(),
            ScopedClient {
                client: client.clone(),
                expires,
            },
        );
        Ok(client)
    }

    fn get(&self, client: &Client, key: &str) -> Result<Option<Vec<u8>>, S3Fail> {
        let bucket = self.bucket.clone();
        let key = key.to_string();
        let client = client.clone();
        self.drive(async move {
            match client.get_object().bucket(bucket).key(key).send().await {
                Ok(output) => match output.body.collect().await {
                    Ok(bytes) => Ok(Some(bytes.into_bytes().to_vec())),
                    Err(err) => Err(S3Fail::Message(err.to_string())),
                },
                Err(err) if is_missing(&err) => Ok(None),
                Err(err) => Err(S3Fail::Message(explain_get(&err))),
            }
        })?
    }

    fn put(
        &self,
        client: &Client,
        key: &str,
        body: &[u8],
        precondition: PutPrecondition,
    ) -> Result<PutOutcome, S3Fail> {
        let bucket = self.bucket.clone();
        let key = key.to_string();
        let body = body.to_vec();
        let client = client.clone();
        self.drive(async move {
            // S3 answers a lost precondition with 412. A 409 means another
            // conditional write was in flight; the upload is retried, and a
            // later 412 is the lost compare-and-swap.
            let mut delay = std::time::Duration::from_millis(25);
            for attempt in 0..6 {
                let mut request = client
                    .put_object()
                    .bucket(bucket.clone())
                    .key(key.clone())
                    .body(ByteStream::from(body.clone()));
                request = match &precondition {
                    PutPrecondition::Create => request.if_none_match("*"),
                    PutPrecondition::Match(tag) => request.if_match(tag),
                };
                match request.send().await {
                    Ok(_) => return Ok(PutOutcome::Stored),
                    Err(err) if is_precondition(&err) => return Ok(PutOutcome::Exists),
                    Err(err) if is_conditional_conflict(&err) && attempt + 1 < 6 => {
                        tokio::time::sleep(delay).await;
                        delay = delay.saturating_mul(2);
                    }
                    Err(err) => return Err(S3Fail::Message(explain_put(&err))),
                }
            }
            Err(S3Fail::Message(
                "s3 put failed: 409 ConditionalRequestConflict".into(),
            ))
        })?
    }

    fn drive<T: Send + 'static>(
        &self,
        fut: impl Future<Output = T> + Send + 'static,
    ) -> Result<T, S3Fail> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.runtime
            .as_ref()
            .expect("memory s3 runtime")
            .spawn(async move {
                let _ = tx.send(fut.await);
            });
        rx.recv()
            .map_err(|_| S3Fail::Message("memory s3 runtime stopped".into()))
    }
}

impl Drop for S3Books {
    fn drop(&mut self) {
        // The engine runtime is still running when a book client is dropped.
        // Dropping this runtime on that thread panics.
        if let Some(runtime) = self.runtime.take() {
            std::thread::spawn(move || drop(runtime));
        }
    }
}

enum PutPrecondition {
    Create,
    Match(String),
}

enum PutOutcome {
    Stored,
    Exists,
}

async fn assume_repo_client(
    _base: Client,
    role_arn: String,
    bucket: String,
    repo_id: String,
    region: String,
    endpoint: Option<String>,
) -> Result<(Client, Instant), S3Fail> {
    use aws_sdk_s3::config::{Credentials, Region};
    use aws_sdk_sts::Client as StsClient;

    // Same default chain the pod uses (Pod Identity / env / profile). The base
    // S3 client was built from that chain in from_env.
    let conf = aws_config::defaults(BehaviorVersion::latest())
        .region(Region::new(region.clone()))
        .load()
        .await;
    let assumed = StsClient::new(&conf)
        .assume_role()
        .role_arn(role_arn)
        .role_session_name(role_session_name(&repo_id))
        .policy(session_policy(&bucket, &repo_id))
        .duration_seconds(900)
        .send()
        .await
        .map_err(|err| S3Fail::Message(format!("sts assume_role failed: {err}")))?;
    let creds = assumed
        .credentials()
        .ok_or_else(|| S3Fail::Message("sts assume_role returned no credentials".into()))?;
    let expires = Instant::now() + Duration::from_secs(800);
    let mut builder = aws_sdk_s3::config::Builder::new()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new(region))
        .credentials_provider(Credentials::new(
            creds.access_key_id(),
            creds.secret_access_key(),
            Some(creds.session_token().to_string()),
            None,
            "cadri-memories-session",
        ))
        .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
        .response_checksum_validation(ResponseChecksumValidation::WhenRequired);
    if let Some(url) = endpoint {
        builder = builder.endpoint_url(url).force_path_style(true);
    }
    Ok((Client::from_conf(builder.build()), expires))
}

fn session_policy(bucket: &str, repo_id: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":["s3:GetObject","s3:GetObjectVersion","s3:PutObject","s3:AbortMultipartUpload","s3:DeleteObject"],"Resource":"arn:aws:s3:::{bucket}/repos/{repo_id}/*"}},{{"Effect":"Allow","Action":["s3:ListBucket","s3:ListBucketVersions"],"Resource":"arn:aws:s3:::{bucket}","Condition":{{"StringLike":{{"s3:prefix":["repos/{repo_id}/*"]}}}}}}]}}"#
    )
}

/// STS `RoleSessionName` is at most 64 characters (`[\w+=,.@-]+`).
/// Personal book repo ids overflow that when prefixed with `cadri-`.
fn role_session_name(repo_id: &str) -> String {
    let candidate = format!("cadri-{repo_id}");
    if candidate.len() <= 64 {
        return candidate;
    }
    let digest = Md5::digest(repo_id.as_bytes());
    let mut hex = String::with_capacity(32);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    format!("cadri-{hex}")
}

fn ref_key(repo_id: &str) -> String {
    format!("repos/{repo_id}/refs/heads/main")
}

fn obj_key(repo_id: &str, sha: &str) -> String {
    format!("repos/{repo_id}/objects/{sha}")
}

fn ref_body(sha: &str) -> Vec<u8> {
    format!("{sha}\n").into_bytes()
}

pub(super) fn etag(bytes: &[u8]) -> String {
    let digest = Md5::digest(bytes);
    let mut quoted = String::with_capacity(34);
    quoted.push('"');
    for byte in digest {
        quoted.push_str(&format!("{byte:02x}"));
    }
    quoted.push('"');
    quoted
}

fn parse_ref(bytes: &[u8]) -> Result<Option<String>, S3Fail> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| S3Fail::Message("memory ref is not utf-8".into()))?;
    let sha = text.trim();
    if sha.is_empty() {
        Ok(None)
    } else if sha.len() == 40 && sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(Some(sha.to_string()))
    } else {
        Err(S3Fail::Message(format!("bad memory ref {sha}")))
    }
}

fn is_missing(err: &SdkError<aws_sdk_s3::operation::get_object::GetObjectError>) -> bool {
    match err {
        SdkError::ServiceError(service) => {
            service.err().is_no_such_key() || service.raw().status().as_u16() == 404
        }
        _ => false,
    }
}

fn explain_get(err: &SdkError<aws_sdk_s3::operation::get_object::GetObjectError>) -> String {
    match err {
        SdkError::ServiceError(service) => format!(
            "s3 get failed: {} {} {}",
            service.raw().status().as_u16(),
            service.err().code().unwrap_or(""),
            service.err().message().unwrap_or("")
        ),
        other => format!("s3 get failed: {other}"),
    }
}

fn explain_put(err: &SdkError<aws_sdk_s3::operation::put_object::PutObjectError>) -> String {
    match err {
        SdkError::ServiceError(service) => format!(
            "s3 put failed: {} {} {}",
            service.raw().status().as_u16(),
            service.err().code().unwrap_or(""),
            service.err().message().unwrap_or("")
        ),
        other => format!("s3 put failed: {other}"),
    }
}

fn is_precondition(err: &SdkError<aws_sdk_s3::operation::put_object::PutObjectError>) -> bool {
    match err {
        SdkError::ServiceError(service) => {
            service.raw().status().as_u16() == 412
                || service.err().code() == Some("PreconditionFailed")
        }
        _ => false,
    }
}

fn is_conditional_conflict(
    err: &SdkError<aws_sdk_s3::operation::put_object::PutObjectError>,
) -> bool {
    match err {
        SdkError::ServiceError(service) => {
            service.raw().status().as_u16() == 409
                || service.err().code() == Some("ConditionalRequestConflict")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn session_policy_is_scoped_to_one_repo() {
        let policy = super::session_policy("bucket", "abcd1234abcd1234abcd1234abcd1234");
        assert!(policy.contains("arn:aws:s3:::bucket/repos/abcd1234abcd1234abcd1234abcd1234/*"));
        assert!(policy.contains(r#""repos/abcd1234abcd1234abcd1234abcd1234/*""#));
        assert!(!policy.contains("repos/*\""));
    }

    #[test]
    fn role_session_name_fits_sts_limit() {
        let team = "org_01M4YRRVVMNRNZHJK5MKZJTYF6_repo_0";
        let personal = "org_01M4YRRVVMNRNZHJK5MKZJTYF6_user_01M4YRRZ0ECV9BC7YZYMPMMFZC_repo_0";
        let team_name = super::role_session_name(team);
        let personal_name = super::role_session_name(personal);
        assert_eq!(team_name, format!("cadri-{team}"));
        assert!(team_name.len() <= 64, "{team_name}");
        assert!(personal_name.len() <= 64, "{personal_name}");
        assert!(personal_name.starts_with("cadri-"));
        assert_ne!(personal_name, format!("cadri-{personal}"));
        assert_eq!(
            super::role_session_name(personal),
            personal_name,
            "hashed name is stable"
        );
    }

    use std::io::{Read, Write};
    use std::net::TcpStream;

    use super::*;

    #[test]
    fn a_conditional_conflict_is_retried() {
        let fake = FakeS3::start();
        fake.conflict_once();
        let books = S3Books::from_client("memories", S3Books::test_client(&fake.endpoint));
        let repo = "abcdef0123456789abcdef0123456789";
        let sha = "a".repeat(40);
        books.put_bundle(repo, &sha, b"bundle").unwrap();
        assert_eq!(books.get_object(repo, &sha).unwrap(), b"bundle");
    }

    #[test]
    fn if_match_rejects_an_unquoted_etag() {
        let fake = FakeS3::start();
        let host = fake.endpoint.trim_start_matches("http://");
        let path = "/memories/repos/id/refs/heads/main";
        let body = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n";
        assert_eq!(put(host, path, body, None, Some("*")), 200);
        let quoted = etag(body);
        let bare = quoted.trim_matches('"').to_string();
        assert_eq!(
            put(
                host,
                path,
                b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n",
                Some(&bare),
                None
            ),
            412
        );
        assert_eq!(
            put(
                host,
                path,
                b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n",
                Some(&quoted),
                None
            ),
            200
        );
    }

    /// Manual check against a real bucket or LocalStack. Not part of `cargo test`.
    #[test]
    #[ignore = "set CADRI_MEMORIES_BUCKET and AWS credentials; CADRI_MEMORIES_S3_ENDPOINT optional"]
    fn real_bucket_compare_and_swap() {
        let bucket = std::env::var("CADRI_MEMORIES_BUCKET").expect("CADRI_MEMORIES_BUCKET");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let books = runtime.block_on(S3Books::from_env(bucket)).unwrap();
        let repo = "0123456789abcdef0123456789abcdef";
        let first = "a".repeat(40);
        let second = "b".repeat(40);
        books.put_bundle(repo, &first, b"bundle").unwrap();
        books.cas_ref(repo, None, &first).unwrap();
        assert!(matches!(
            books.cas_ref(repo, None, &second),
            Err(S3Fail::Conflict)
        ));
        books.cas_ref(repo, Some(&first), &second).unwrap();
        assert_eq!(
            books.read_ref(repo).unwrap().as_deref(),
            Some(second.as_str())
        );
        assert_eq!(books.get_object(repo, &first).unwrap(), b"bundle");
    }

    fn put(
        host: &str,
        path: &str,
        body: &[u8],
        if_match: Option<&str>,
        if_none: Option<&str>,
    ) -> u16 {
        let mut stream = TcpStream::connect(host).unwrap();
        let mut extra = String::new();
        if let Some(tag) = if_match {
            extra.push_str(&format!("If-Match: {tag}\r\n"));
        }
        if let Some(tag) = if_none {
            extra.push_str(&format!("If-None-Match: {tag}\r\n"));
        }
        let header = format!(
            "PUT {path} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(header.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        let text = String::from_utf8_lossy(&raw);
        text.lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap()
    }
}
