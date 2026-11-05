//! Memory book benchmarks. Every test is ignored; run `scripts/bench-memory`.
//!
//! One FakeS3 with `BENCH_S3_LATENCY_MS` (default 30) of delay per request stands
//! in for the durable store. Books are seeded through the HTTP edit endpoint, so
//! the same benchmark runs on old and new engine code.

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use serde_json::{json, Value};

use cadri_core::auth::NoAuth;
use cadri_core::db;
use cadri_core::memory::testing::{assemble, FakeS3, MemoryService, S3Books};

#[derive(Clone, Copy)]
enum Corpus {
    Small,
    Large,
}

impl Corpus {
    fn name(self) -> &'static str {
        match self {
            Corpus::Small => "small",
            Corpus::Large => "large",
        }
    }

    fn files(self) -> usize {
        match self {
            Corpus::Small => 40,
            Corpus::Large => 1000,
        }
    }

    fn lines(self) -> usize {
        match self {
            Corpus::Small => 400,
            Corpus::Large => 40,
        }
    }

    fn path(self, index: usize) -> String {
        match self {
            Corpus::Small => format!("notes/{index}.md"),
            Corpus::Large => format!("dir{}/file{index}.md", index % 20),
        }
    }
}

struct Bench {
    root: PathBuf,
    fake: FakeS3,
    config: PathBuf,
}

impl Bench {
    fn new() -> Self {
        let root = temp();
        let fake = FakeS3::start();
        let config = git_config(&root);
        Self { root, fake, config }
    }

    /// A fresh server process state over the shared bucket: new service, new
    /// local repo directory.
    fn server(&self, name: &str) -> SocketAddr {
        let client = S3Books::test_client(&self.fake.endpoint);
        let dir = self.root.join(name);
        fs::create_dir_all(&dir).unwrap();
        let service =
            MemoryService::open_s3(dir.clone(), false, S3Books::from_client("memories", client));
        let app = assemble(
            db::open(&dir.join("transcripts.db")).unwrap(),
            Arc::new(NoAuth),
            None,
            service,
        );
        let runtime_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        runtime_listener.set_nonblocking(true).unwrap();
        let addr = runtime_listener.local_addr().unwrap();
        tokio::spawn(serve(runtime_listener, app));
        addr
    }

    /// Seeds with latency off so setup stays quick; latency applies to what is measured.
    fn seed(&self, addr: &SocketAddr, corpus: Corpus) {
        self.fake.set_latency(Duration::ZERO);
        let line = "the team keeps this note so agents can reuse it.\n";
        let body = line.repeat(corpus.lines());
        let changes: Vec<Value> = (0..corpus.files())
            .map(|index| json!({ "op": "write", "path": corpus.path(index), "content": body }))
            .collect();
        post_commit(addr, "seed", changes).expect("seed commit");
        let readme = json!({ "op": "write", "path": "README.md", "content": "# Team book\n" });
        post_commit(addr, "readme", vec![readme]).expect("readme commit");
        for round in 0..8 {
            let index = round % corpus.files();
            let change = json!({
                "op": "write",
                "path": corpus.path(index),
                "content": format!("{body}\nedit {round}\n"),
            });
            post_commit(addr, &format!("edit {round}"), vec![change]).expect("edit commit");
        }
        self.fake.set_latency(latency());
    }

    fn finish(self) {
        let _ = fs::remove_dir_all(self.root);
    }
}

async fn serve(listener: std::net::TcpListener, app: Router) {
    let listener = tokio::net::TcpListener::from_std(listener).unwrap();
    axum::serve(listener, app).await.unwrap();
}

fn latency() -> Duration {
    let ms = std::env::var("BENCH_S3_LATENCY_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(30);
    Duration::from_millis(ms)
}

/// One seeded book per corpus serves `page_load`, `cold_read`, and (for the
/// large corpus) `clone`, because seeding the large book is slow on old code.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark; run scripts/bench-memory"]
async fn reads_small() {
    reads(Corpus::Small).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark; run scripts/bench-memory"]
async fn reads_large() {
    reads(Corpus::Large).await;
}

async fn reads(corpus: Corpus) {
    let bench = Bench::new();
    let addr = bench.server("main");
    let seeding = Instant::now();
    bench.seed(&addr, corpus);
    println!(
        "bench seed_{} seconds={:.1}",
        corpus.name(),
        seeding.elapsed().as_secs_f64()
    );
    page_load(&bench, &addr, corpus);
    cold_read(&bench, corpus);
    if matches!(corpus, Corpus::Large) {
        clone_book(&bench, &addr);
    }
    bench.finish();
}

fn page_load(bench: &Bench, addr: &SocketAddr, corpus: Corpus) {
    let name = format!("page_load_{}", corpus.name());
    get(addr, "/api/memories");
    let before = bench.fake.requests();
    let mut times = Vec::new();
    let mut bytes = 0;
    for _ in 0..50 {
        let started = Instant::now();
        let (status, body) = http(addr, "GET", "/api/memories", None);
        times.push(started.elapsed());
        assert_eq!(status, 200);
        bytes = body.len();
    }
    let after = bench.fake.requests();
    let mut out = Results::new(&name);
    out.ms("legacy_p50_ms", percentile(&times, 50.0));
    out.ms("legacy_p95_ms", percentile(&times, 95.0));
    out.num("legacy_bytes", bytes as f64);
    out.num("fake_s3_gets_during_50_reads", (after.0 - before.0) as f64);
    out.num("fake_s3_puts_during_50_reads", (after.1 - before.1) as f64);

    let (status, tree) = http(addr, "GET", "/api/memories/tree", None);
    if status != 404 {
        assert_eq!(status, 200);
        let mut times = Vec::new();
        let mut tree_bytes = 0;
        for _ in 0..50 {
            let started = Instant::now();
            let (_, body) = http(addr, "GET", "/api/memories/tree", None);
            times.push(started.elapsed());
            tree_bytes = body.len();
        }
        out.ms("tree_p50_ms", percentile(&times, 50.0));
        out.ms("tree_p95_ms", percentile(&times, 95.0));
        out.num("tree_bytes", tree_bytes as f64);
        if let Some(sha) = readme_sha(&tree) {
            let path = format!("/api/memories/blob/{sha}");
            let mut times = Vec::new();
            for _ in 0..50 {
                let started = Instant::now();
                let (status, _) = http(addr, "GET", &path, None);
                times.push(started.elapsed());
                assert_eq!(status, 200);
            }
            out.ms("blob_p50_ms", percentile(&times, 50.0));
            out.ms("blob_p95_ms", percentile(&times, 95.0));
        }
    }
    out.write();
}

fn cold_read(bench: &Bench, corpus: Corpus) {
    let cold = bench.server("srv-cold-read");
    let started = Instant::now();
    let (status, body) = http(&cold, "GET", "/api/memories", None);
    let elapsed = started.elapsed();
    assert_eq!(status, 200);
    let mut out = Results::new(&format!("cold_read_{}", corpus.name()));
    out.ms("first_read_ms", elapsed);
    out.num("bytes", body.len() as f64);
    out.write();
}

fn clone_book(bench: &Bench, warm: &SocketAddr) {
    let mut out = Results::new("clone_large");
    let url = format!("http://{warm}/git/team.git");
    let started = Instant::now();
    assert_ok(git(
        &bench.root,
        &bench.config,
        &["clone", &url, "warm-clone"],
    ));
    out.ms("warm_clone_ms", started.elapsed());

    let cold = bench.server("srv-cold-clone");
    let url = format!("http://{cold}/git/team.git");
    let started = Instant::now();
    assert_ok(git(
        &bench.root,
        &bench.config,
        &["clone", &url, "cold-clone"],
    ));
    out.ms("cold_clone_ms", started.elapsed());
    out.write();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark; run scripts/bench-memory"]
async fn push_latency() {
    let bench = Bench::new();
    let addr = bench.server("main");
    bench.seed(&addr, Corpus::Large);
    let url = format!("http://{addr}/git/team.git");
    let dir = bench.root.join("client");
    assert_ok(git(&bench.root, &bench.config, &["clone", &url, "client"]));
    let path = Corpus::Large.path(0);
    let mut times = Vec::new();
    let scenario_start = Instant::now();
    // Old code takes seconds per push; stop at the budget so the suite stays short.
    for cycle in 0..50 {
        if cycle > 0 && scenario_start.elapsed() > Duration::from_secs(150) {
            break;
        }
        let mut lines: Vec<String> = fs::read_to_string(dir.join(&path))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        for line in lines.iter_mut().take(100) {
            *line = format!("cycle {cycle} rewrote this line");
        }
        while lines.len() < 100 {
            lines.push(format!("cycle {cycle} extra line"));
        }
        fs::write(dir.join(&path), lines.join("\n") + "\n").unwrap();
        commit_all(&dir, &bench.config, &format!("cycle {cycle}"));
        let started = Instant::now();
        let pushed = git(&dir, &bench.config, &["push", &url, "HEAD:refs/heads/main"]);
        times.push(started.elapsed());
        assert_ok(pushed);
    }
    let mut out = Results::new("push_latency_large");
    out.num("cycles", times.len() as f64);
    out.ms("push_p50_ms", percentile(&times, 50.0));
    out.ms("push_p95_ms", percentile(&times, 95.0));
    out.ms("push_p99_ms", percentile(&times, 99.0));
    out.write();
    bench.finish();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark; run scripts/bench-memory"]
async fn push_contention() {
    for clients in [5usize, 10] {
        let bench = Bench::new();
        let addr = bench.server("main");
        bench.seed(&addr, Corpus::Small);
        let url = format!("http://{addr}/git/team.git");
        let run = Duration::from_secs(30);
        let samples: Arc<Mutex<Vec<(bool, Duration)>>> = Arc::new(Mutex::new(Vec::new()));
        let started = Instant::now();
        let handles: Vec<_> = (0..clients)
            .map(|client| {
                let url = url.clone();
                let config = bench.config.clone();
                let root = bench.root.clone();
                let samples = samples.clone();
                std::thread::spawn(move || {
                    let name = format!("client{client}");
                    let dir = root.join(&name);
                    assert_ok(git(&root, &config, &["clone", &url, &name]));
                    let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ (client as u64 + 1));
                    let mut counter = 0u64;
                    while started.elapsed() < run {
                        counter += 1;
                        let file = if rng.next() % 100 < 20 {
                            Corpus::Small.path(0)
                        } else {
                            Corpus::Small.path(1 + client)
                        };
                        fs::write(
                            dir.join(&file),
                            format!("client {client} edit {counter}\n").repeat(20),
                        )
                        .unwrap();
                        commit_all(&dir, &config, &format!("c{client} e{counter}"));
                        let attempt_start = Instant::now();
                        let mut ok = false;
                        for _ in 0..3 {
                            let pushed =
                                git(&dir, &config, &["push", &url, "HEAD:refs/heads/main"]);
                            if pushed.status.success() {
                                ok = true;
                                break;
                            }
                            // Keep our side on conflict, like the load test.
                            git(
                                &dir,
                                &config,
                                &["pull", "--rebase", "-X", "theirs", &url, "main"],
                            );
                            if git(&dir, &config, &["status", "--porcelain=v1"])
                                .stdout
                                .starts_with(b"UU")
                            {
                                git(&dir, &config, &["rebase", "--abort"]);
                            }
                        }
                        samples.lock().unwrap().push((ok, attempt_start.elapsed()));
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        let elapsed = started.elapsed();
        let samples = samples.lock().unwrap();
        let good: Vec<Duration> = samples.iter().filter(|s| s.0).map(|s| s.1).collect();
        let mut out = Results::new(&format!("push_contention_{clients}_clients"));
        out.num("commits_per_s", good.len() as f64 / elapsed.as_secs_f64());
        out.num(
            "success_rate",
            good.len() as f64 / samples.len().max(1) as f64,
        );
        out.num("attempted_commits", samples.len() as f64);
        out.ms("p50_ms", percentile(&good, 50.0));
        out.ms("p95_ms", percentile(&good, 95.0));
        out.write();
        bench.finish();
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark; run scripts/bench-memory"]
async fn edit_api() {
    for rate in [10.0f64, 50.0] {
        let bench = Bench::new();
        let addr = bench.server("main");
        bench.seed(&addr, Corpus::Small);
        let threads = 10usize;
        let run = Duration::from_secs(30);
        let interval = Duration::from_secs_f64(threads as f64 / rate);
        let samples: Arc<Mutex<Vec<(bool, Duration)>>> = Arc::new(Mutex::new(Vec::new()));
        let started = Instant::now();
        let handles: Vec<_> = (0..threads)
            .map(|thread| {
                let samples = samples.clone();
                std::thread::spawn(move || {
                    let mut next = started + interval.mul_f64(thread as f64 / threads as f64);
                    let mut counter = 0u64;
                    while started.elapsed() < run {
                        let now = Instant::now();
                        if next > now {
                            std::thread::sleep(next - now);
                        }
                        next += interval;
                        counter += 1;
                        let change = json!({
                            "op": "write",
                            "path": format!("edits/thread{thread}.md"),
                            "content": format!("thread {thread} write {counter}\n"),
                        });
                        let begin = Instant::now();
                        let ok = post_commit(&addr, "edit", vec![change]).is_some();
                        samples.lock().unwrap().push((ok, begin.elapsed()));
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        let elapsed = started.elapsed();
        let samples = samples.lock().unwrap();
        let good: Vec<Duration> = samples.iter().filter(|s| s.0).map(|s| s.1).collect();
        let mut out = Results::new(&format!("edit_api_{}_per_s", rate as u64));
        out.num(
            "success_rate",
            good.len() as f64 / samples.len().max(1) as f64,
        );
        out.num("achieved_per_s", good.len() as f64 / elapsed.as_secs_f64());
        out.ms("p50_ms", percentile(&good, 50.0));
        out.ms("p95_ms", percentile(&good, 95.0));
        out.write();
        bench.finish();
    }
}

fn readme_sha(tree: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(tree).ok()?;
    value["files"]
        .as_array()?
        .iter()
        .find(|file| file["path"] == "README.md")
        .and_then(|file| file["blob"].as_str().or(file["sha"].as_str()))
        .map(str::to_string)
}

struct Results {
    scenario: String,
    values: serde_json::Map<String, Value>,
}

impl Results {
    fn new(scenario: &str) -> Self {
        Self {
            scenario: scenario.to_string(),
            values: serde_json::Map::new(),
        }
    }

    fn num(&mut self, metric: &str, value: f64) {
        let value = (value * 1000.0).round() / 1000.0;
        println!("bench {} {metric}={value}", self.scenario);
        self.values.insert(metric.to_string(), json!(value));
    }

    fn ms(&mut self, metric: &str, value: Duration) {
        self.num(metric, value.as_secs_f64() * 1000.0);
    }

    fn write(self) {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/memory-bench");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("{}.json", self.scenario)),
            serde_json::to_vec_pretty(&Value::Object(self.values)).unwrap(),
        )
        .unwrap();
    }
}

fn percentile(times: &[Duration], pct: f64) -> Duration {
    if times.is_empty() {
        return Duration::ZERO;
    }
    let mut sorted = times.to_vec();
    sorted.sort();
    let index = ((pct / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[index.clamp(1, sorted.len()) - 1]
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn post_commit(addr: &SocketAddr, message: &str, changes: Vec<Value>) -> Option<Value> {
    let body = serde_json::to_vec(&json!({ "message": message, "changes": changes })).unwrap();
    let (status, payload) = http(addr, "POST", "/api/memories/commits", Some(&body));
    if status == 200 {
        Some(serde_json::from_slice(&payload).unwrap_or(Value::Null))
    } else {
        eprintln!(
            "commit failed {status}: {}",
            String::from_utf8_lossy(&payload)
        );
        None
    }
}

fn get(addr: &SocketAddr, path: &str) -> Vec<u8> {
    let (status, body) = http(addr, "GET", path, None);
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    body
}

fn http(addr: &SocketAddr, method: &str, path: &str, body: Option<&[u8]>) -> (u16, Vec<u8>) {
    let body = body.unwrap_or(b"");
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(120)))
        .unwrap();
    let header = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap_or_else(|| panic!("short HTTP response: {}", String::from_utf8_lossy(&raw)));
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let status: u16 = head
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    (status, raw[split + 4..].to_vec())
}

fn git_config(root: &Path) -> PathBuf {
    let config = root.join("gitconfig");
    fs::write(
        &config,
        "[user]\n\tname = Default\n\temail = default@cadri.local\n",
    )
    .unwrap();
    config
}

struct GitOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn git(dir: &Path, config: &Path, args: &[&str]) -> GitOutput {
    fs::create_dir_all(dir).ok();
    let output = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .args(args)
        .output()
        .unwrap();
    GitOutput {
        status: output.status,
        stdout: output.stdout,
        stderr: output.stderr,
    }
}

fn assert_ok(output: GitOutput) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn commit_all(dir: &Path, config: &Path, message: &str) {
    assert_ok(git(dir, config, &["add", "-A"]));
    let message_path = dir.join(".git/COMMIT_EDITMSG_CADRI");
    fs::write(
        &message_path,
        format!(
            "{message}\n\nCadri-Author-Kind: human\nCadri-Author-Id: default\nCadri-Author-Label: default\n"
        ),
    )
    .unwrap();
    assert_ok(git(
        dir,
        config,
        &["commit", "--allow-empty", "-F", ".git/COMMIT_EDITMSG_CADRI"],
    ));
}

fn temp() -> PathBuf {
    static TEMPS: AtomicU64 = AtomicU64::new(0);
    let n = TEMPS.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "cadri-bench-{}-{}-{}",
        std::process::id(),
        n,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}
