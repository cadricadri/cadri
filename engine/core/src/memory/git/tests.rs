//! Unit tests for the memory git service.

use std::fs;
use std::path::{Path, PathBuf};

use crate::memory::s3::S3Books;
use crate::memory::types::{Author, Change};

use super::protocol::pkt;
use super::service::{Backend, MemoryService};

fn temp() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static TEMPS: AtomicU64 = AtomicU64::new(0);
    let n = TEMPS.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "cadri-memory-{}-{}-{}",
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

fn author() -> (&'static str, &'static str) {
    ("ada", "Ada")
}

fn write(path: &str, content: &str) -> Change {
    Change::Write {
        path: path.to_string(),
        content: content.to_string(),
    }
}

#[test]
fn service_header_is_the_classic_pkt_line() {
    let header = pkt("# service=git-upload-pack\n");
    assert_eq!(header, b"001e# service=git-upload-pack\n");
}

#[test]
fn ui_commit_round_trip_stores_a_large_book() {
    let root = temp();
    let service = MemoryService::open_local(&root, false);
    let repo = "org_demo_repo_0";
    let (user, label) = author();
    let empty = service.read_store(repo).unwrap();
    assert!(empty.files.is_empty());
    let saved = service
        .commit(
            repo,
            user,
            label,
            "Add a note",
            &[write("notes.md", "hello")],
        )
        .unwrap();
    assert_eq!(saved.files[0].content, "hello");
    assert!(saved.commits.is_empty());
    assert!(saved.files[0].blame.is_empty());
    let blame = service.read_blame(repo, "notes.md").unwrap();
    assert_eq!(
        blame[0].author,
        Author::Human {
            user_id: "ada".into(),
            label: "Ada".into(),
        }
    );
    let log = service.read_log(repo).unwrap();
    assert_eq!(log[0].message, "Add a note");
    assert_eq!(log.len(), 1);
    assert_eq!(blame[0].commit_id, log[0].id);
    service
        .commit(
            repo,
            user,
            label,
            "Add a note",
            &[write("notes.md", "hello")],
        )
        .unwrap();
    assert_eq!(service.read_log(repo).unwrap().len(), 1);

    let folder = service
        .commit(
            repo,
            user,
            label,
            "Create folder notes",
            &[write("notes/.keep", "")],
        )
        .unwrap();
    assert_eq!(folder.directories, vec!["notes".to_string()]);
    assert!(folder.files.iter().all(|file| file.path != "notes/.keep"));

    let long = service
        .commit(
            repo,
            user,
            label,
            "a long note",
            &[write("big.md", &"a\n".repeat(20_001))],
        )
        .unwrap();
    assert!(long.files.iter().any(|file| file.path == "big.md"));
    let mut many = Vec::new();
    for index in 0..101 {
        many.push(write(&format!("extra/{index}.md"), "x"));
    }
    let wide = service
        .commit(repo, user, label, "many notes", &many)
        .unwrap();
    assert!(wide.files.len() > 100);
    assert_eq!(service.read_log(repo).unwrap()[0].message, "many notes");

    let clash = service.commit(repo, user, label, "file", &[write("notes", "x")]);
    assert!(matches!(clash, Err(err) if err.message == "invalid memory path"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn two_caches_share_one_ref_and_both_changes_land() {
    let root = temp();
    let store = root.join("store");
    let left = MemoryService::new(
        root.join("left"),
        Backend::Directory(store.clone()),
        root.join("sl"),
        false,
    );
    let right = MemoryService::new(
        root.join("right"),
        Backend::Directory(store),
        root.join("sr"),
        false,
    );
    let repo = "org_pair_repo_0";
    let (user, label) = author();
    let left_commit = {
        let left = left.clone();
        std::thread::spawn(move || left.commit(repo, user, label, "left", &[write("left.md", "L")]))
    };
    let right_commit = std::thread::spawn(move || {
        right.commit(repo, user, label, "right", &[write("right.md", "R")])
    });
    left_commit.join().unwrap().unwrap();
    right_commit.join().unwrap().unwrap();
    let fresh = MemoryService::new(
        root.join("fresh"),
        Backend::Directory(root.join("store")),
        root.join("sf"),
        false,
    );
    let store = fresh.read_store(repo).unwrap();
    let paths: Vec<&str> = store.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, vec!["left.md", "right.md"]);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_legacy_book_migrates_and_a_missing_bundle_fails_with_a_clear_error() {
    let root = temp();
    let history = legacy_history(
        &root.join("legacy"),
        &[&[("note.md", "one")], &[("note.md", "two")]],
    );
    let current = &history[1];
    let repo = "org_cache_repo_0";
    legacy_layout(&root.join("data/store"), repo, current, true);
    let service = MemoryService::open_local(root.join("data"), false);
    assert_eq!(service.read_store(repo).unwrap().files[0].content, "two");
    assert_eq!(service.read_log(repo).unwrap().len(), 2);

    legacy_layout(&root.join("other/store"), repo, current, false);
    let restarted = MemoryService::open_local(root.join("other"), false);
    let err = restarted.read_store(repo).expect_err("missing bundle");
    assert_eq!(err.status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn s3_legacy_book_migrates_and_a_missing_bundle_fails_with_a_clear_error() {
    let fake = crate::memory::s3::FakeS3::start();
    let root = temp();
    let history = legacy_history(
        &root.join("legacy"),
        &[&[("note.md", "one")], &[("note.md", "two")]],
    );
    let current = &history[1];
    let books = S3Books::from_client("memories", S3Books::test_client(&fake.endpoint));
    let repo = "org_cache_repo_0";
    legacy_s3(&books, repo, current, true);
    let reader = MemoryService::open_s3(root.join("reader"), false, books);
    assert_eq!(reader.read_store(repo).unwrap().files[0].content, "two");
    assert!(fake
        .keys()
        .contains(&format!("repos/{repo}/checkpoint/00000000000000000000")));

    let other = "org_other_repo_0";
    let books = S3Books::from_client("memories", S3Books::test_client(&fake.endpoint));
    legacy_s3(&books, other, current, false);
    let restarted = MemoryService::open_s3(root.join("restarted"), false, books);
    let err = restarted.read_store(other).expect_err("missing bundle");
    assert_eq!(err.status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn ui_commit_returns_conflict_when_its_retry_also_loses_the_log_create() {
    let root = temp();
    let service = MemoryService::open_local(&root, false);
    let repo = "org_test_repo_0";
    let (user, label) = author();
    service.fail_next_log_create(2);
    let err = service
        .commit(
            repo,
            user,
            label,
            "Add a note",
            &[write("note.md", "hello")],
        )
        .unwrap_err();
    assert_eq!(err.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(err.message, "memory book changed");
    let store = service.read_store(repo).unwrap();
    assert!(store.files.is_empty());
    assert!(store.commits.is_empty());
    let stage = root
        .join("repos")
        .join(format!("{repo}.git"))
        .join("refs/cadri/stage");
    assert!(!stage.exists());
    service.fail_next_log_create(1);
    let store = service
        .commit(
            repo,
            user,
            label,
            "Add a note",
            &[write("note.md", "hello")],
        )
        .unwrap();
    assert_eq!(store.files.len(), 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_failed_bundle_put_removes_the_staging_ref() {
    let fake = crate::memory::s3::FakeS3::start();
    fake.fail_next_object_put();
    let root = temp();
    let service = MemoryService::open_s3(
        &root,
        false,
        S3Books::from_client("memories", S3Books::test_client(&fake.endpoint)),
    );
    let repo = "org_demo_repo_0";
    let (user, label) = author();
    let err = service
        .commit(
            repo,
            user,
            label,
            "Add a note",
            &[write("note.md", "hello")],
        )
        .unwrap_err();
    assert_eq!(err.status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    let stage = root
        .join("repos")
        .join(format!("{repo}.git"))
        .join("refs/cadri/stage");
    assert!(!stage.exists());
    assert!(service.read_store(repo).unwrap().commits.is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn one_commit_produces_the_same_bundle_twice() {
    let root = temp();
    let service = MemoryService::open_local(&root, false);
    let repo = "org_pair_repo_0";
    let (user, label) = author();
    service
        .commit(
            repo,
            user,
            label,
            "Add a note",
            &[write("note.md", "hello")],
        )
        .unwrap();
    let sha = service.read_log(repo).unwrap()[0].id.clone();
    let git_dir = root.join("repos").join(format!("{repo}.git"));
    let stored = fs::read(log_file(&root.join("store"), repo, 1)).unwrap();
    let first = rebuild_bundle(&git_dir, &sha);
    let second = rebuild_bundle(&git_dir, &sha);
    assert_eq!(stored, first);
    assert_eq!(first, second);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn s3_log_creates_skip_checksums() {
    let fake = crate::memory::s3::FakeS3::start();
    let root = temp();
    let service = counted_s3_service(&root, &fake);
    let repo = "org_cache_repo_0";
    let (user, label) = author();
    service
        .commit(repo, user, label, "first", &[write("note.md", "one")])
        .unwrap();
    service
        .commit(repo, user, label, "second", &[write("note.md", "two")])
        .unwrap();
    assert_eq!(service.read_tip(repo).unwrap().1, 2);
    assert!(!fake.saw_checksum());
    let _ = fs::remove_dir_all(root);
}

fn rebuild_bundle(git_dir: &Path, sha: &str) -> Vec<u8> {
    let bundle = git_dir.join(format!("rebuilt-{sha}.bundle"));
    let _ = fs::remove_file(&bundle);
    let update = std::process::Command::new("git")
        .arg("-C")
        .arg(git_dir)
        .args(["update-ref", "refs/cadri/stage", sha])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        update.status.success(),
        "{}",
        String::from_utf8_lossy(&update.stderr)
    );
    let created = std::process::Command::new("git")
        .arg("-C")
        .arg(git_dir)
        .args([
            "bundle",
            "create",
            &bundle.to_string_lossy(),
            "refs/cadri/stage",
        ])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    fs::read(&bundle).unwrap()
}

#[test]
fn a_reader_does_not_wait_for_the_book_lock() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    let root = temp();
    let service = MemoryService::open_local(&root, false);
    let repo = "org_lock_repo_0";
    let (user, label) = author();
    service
        .commit(
            repo,
            user,
            label,
            "Add a note",
            &[write("note.md", "hello")],
        )
        .unwrap();
    let entered = std::sync::Arc::new(AtomicBool::new(false));
    let flag = std::sync::Arc::clone(&entered);
    let waiting = service.clone();
    let holder = std::thread::spawn(move || {
        waiting.hold_writer(repo, Duration::from_millis(1200), &flag);
    });
    let start = Instant::now();
    while !entered.load(Ordering::SeqCst) {
        if start.elapsed() > Duration::from_secs(2) {
            panic!("writer did not take the book lock");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let read_at = Instant::now();
    let store = service.read_store(repo).unwrap();
    let elapsed = read_at.elapsed();
    holder.join().unwrap();
    assert_eq!(store.files[0].content, "hello");
    assert!(
        elapsed < Duration::from_millis(600),
        "read waited on the writer: {elapsed:?}"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_clone_does_not_wait_for_a_writer() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    let root = temp();
    let service = MemoryService::open_local(&root, false);
    let repo = "org_clone_repo_0";
    let (user, label) = author();
    service
        .commit(
            repo,
            user,
            label,
            "Add a note",
            &[write("note.md", "hello")],
        )
        .unwrap();
    // A load takes the writer, so load first; the hold below must not delay the clone.
    service.read_store(repo).unwrap();
    let git_dir = root.join("repos").join(format!("{repo}.git"));
    let parsed = std::process::Command::new("git")
        .arg("-C")
        .arg(&git_dir)
        .args(["rev-parse", "refs/heads/main"])
        .output()
        .unwrap();
    let tip = String::from_utf8(parsed.stdout).unwrap().trim().to_string();
    let hidden = std::process::Command::new("git")
        .arg("-C")
        .arg(&git_dir)
        .args(["update-ref", "refs/cadri/stage", &tip])
        .status()
        .unwrap();
    assert!(hidden.success());

    let entered = std::sync::Arc::new(AtomicBool::new(false));
    let flag = std::sync::Arc::clone(&entered);
    let waiting = service.clone();
    let holder = std::thread::spawn(move || {
        waiting.hold_writer(repo, Duration::from_millis(1500), &flag);
    });
    while !entered.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(5));
    }
    let started = Instant::now();
    let ad = service
        .serve_git(
            repo,
            "info/refs",
            false,
            Some("git-upload-pack"),
            &[],
            "ada",
        )
        .unwrap();
    let mut want = pkt(&format!("want {tip}\n"));
    want.extend_from_slice(b"0000");
    want.extend(pkt("done\n"));
    let fetched = service
        .serve_git(repo, "git-upload-pack", true, None, &want, "ada")
        .unwrap();
    let receive_ad = service
        .serve_git(
            repo,
            "info/refs",
            false,
            Some("git-receive-pack"),
            &[],
            "ada",
        )
        .unwrap();
    let elapsed = started.elapsed();
    holder.join().unwrap();
    let super::protocol::GitHttp::Ready { body: ad, .. } = ad;
    let super::protocol::GitHttp::Ready { body: fetched, .. } = fetched;
    let super::protocol::GitHttp::Ready {
        body: receive_ad, ..
    } = receive_ad;
    let ad = String::from_utf8_lossy(&ad);
    assert!(ad.contains("refs/heads/main"));
    assert!(!ad.contains("refs/cadri"), "internal refs were advertised");
    assert!(fetched.windows(4).any(|w| w == b"PACK"));
    assert!(String::from_utf8_lossy(&receive_ad).contains(&tip));
    assert!(
        elapsed < Duration::from_millis(700),
        "clone waited on the writer: {elapsed:?}"
    );
    let _ = fs::remove_dir_all(root);
}

/// Times the old page-load view against the tree read, blob batching, and a read
/// that overlaps a held write lock. Run with:
/// `cargo test --release -p cadri-core --lib memory_read_benchmark -- --ignored --nocapture`
#[test]
#[ignore = "performance benchmark; run in release"]
fn memory_read_benchmark() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::git_cli_exec::Repo;
    use super::snapshot::snapshot_from_repo;
    use super::tree::{cat_blobs, cat_blobs_serial, list_tree};
    use super::view::view_legacy;

    const FILES: usize = 40;
    const LINES: usize = 400;
    let root = temp();
    let service = MemoryService::open_local(&root, false);
    let repo_id = "org_bench_repo_0";
    let (user, label) = author();
    let body = "the team keeps this note so agents can reuse it.\n".repeat(LINES);
    let mut created = Vec::new();
    for index in 0..FILES {
        created.push(write(&format!("notes/{index}.md"), &body));
    }
    service
        .commit(repo_id, user, label, "seed", &created)
        .unwrap();
    for round in 0..8 {
        let mut edits = Vec::new();
        for slot in 0..4 {
            let index = (round * 4 + slot) % FILES;
            edits.push(write(
                &format!("notes/{index}.md"),
                &format!("{body}\nedit {round}\n"),
            ));
        }
        service
            .commit(repo_id, user, label, &format!("edit {round}"), &edits)
            .unwrap();
    }

    let git = Repo {
        git_dir: root.join("repos").join(format!("{repo_id}.git")),
        scratch: root.join("scratch").join(repo_id),
        jail: false,
    };
    let shas: Vec<String> = list_tree(&git, "HEAD")
        .unwrap()
        .into_iter()
        .map(|(_, sha, _)| sha)
        .collect();

    let serial = time(|| {
        cat_blobs_serial(&git, &shas).unwrap();
    });
    let batched = time(|| {
        cat_blobs(&git, &shas).unwrap();
    });
    let legacy = time(|| {
        view_legacy(&git).unwrap();
    });
    let tip = list_tip(&git);
    let tree = time(|| {
        snapshot_from_repo(&git, Some(&tip), 0, None).unwrap();
    });
    let legacy_store = view_legacy(&git).unwrap();
    let tree_store = snapshot_from_repo(&git, Some(&tip), 0, None)
        .unwrap()
        .to_store();
    let log = time(|| {
        service.read_log(repo_id).unwrap();
    });
    let one_blame = time(|| {
        service.read_blame(repo_id, "notes/0.md").unwrap();
    });
    let warm = time(|| {
        service.read_store(repo_id).unwrap();
    });
    service.read_store(repo_id).unwrap();
    let warm_again = time(|| {
        service.read_store(repo_id).unwrap();
    });

    let entered = std::sync::Arc::new(AtomicBool::new(false));
    let flag = std::sync::Arc::clone(&entered);
    let waiting = service.clone();
    let holder = std::thread::spawn(move || {
        waiting.hold_writer(repo_id, Duration::from_millis(800), &flag);
    });
    while !entered.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(5));
    }
    let overlapped = time(|| {
        service.read_store(repo_id).unwrap();
    });
    holder.join().unwrap();

    let legacy_bytes = serde_json::to_vec(&legacy_store).unwrap().len();
    let tree_bytes = serde_json::to_vec(&tree_store).unwrap().len();
    println!(
        "bench files={FILES} lines_per_file={LINES} blobs={}",
        shas.len()
    );
    println!("bench serial_cat_file_ms={}", ms(serial));
    println!("bench batch_cat_file_ms={}", ms(batched));
    println!("bench legacy_full_view_ms={}", ms(legacy));
    println!("bench tree_view_ms={}", ms(tree));
    println!("bench history_ms={}", ms(log));
    println!("bench one_file_blame_ms={}", ms(one_blame));
    println!("bench warm_read_store_ms={}", ms(warm));
    println!("bench warm_read_store_again_ms={}", ms(warm_again));
    println!("bench read_while_writer_holds_lock_ms={}", ms(overlapped));
    println!("bench writer_hold_ms=800");
    println!("bench legacy_json_bytes={legacy_bytes}");
    println!("bench tree_json_bytes={tree_bytes}");
    assert_eq!(tree_store.files.len(), FILES);
    assert!(tree_store.commits.is_empty());
    assert!(legacy_store.commits.len() > 1);
    assert!(
        batched < serial,
        "batch {batched:?} was not faster than per-file {serial:?}"
    );
    assert!(
        tree < legacy,
        "tree {tree:?} was not faster than the full view {legacy:?}"
    );
    assert!(
        overlapped < Duration::from_millis(500),
        "read waited on the writer: {overlapped:?}"
    );
    let _ = fs::remove_dir_all(root);
}

fn list_tip(git: &super::git_cli_exec::Repo) -> String {
    super::git_cli_exec::local_head(git).unwrap().unwrap()
}

fn time(work: impl FnOnce()) -> std::time::Duration {
    let start = std::time::Instant::now();
    work();
    start.elapsed()
}

fn ms(duration: std::time::Duration) -> u128 {
    duration.as_millis()
}

fn counted_s3_service(root: &Path, fake: &crate::memory::s3::FakeS3) -> MemoryService {
    MemoryService::open_s3(
        root,
        false,
        S3Books::from_client("memories", S3Books::test_client(&fake.endpoint)),
    )
}

#[test]
fn reads_make_no_s3_requests_once_the_book_is_loaded() {
    let fake = crate::memory::s3::FakeS3::start();
    let root = temp();
    let service = counted_s3_service(&root, &fake);
    let repo = "org_reads_repo_0";
    let (user, label) = author();
    service
        .commit(
            repo,
            user,
            label,
            "Add a note",
            &[write("note.md", "hello")],
        )
        .unwrap();
    service.read_store(repo).unwrap();
    let before = fake.requests();
    for _ in 0..20 {
        let store = service.read_store(repo).unwrap();
        assert_eq!(store.files[0].content, "hello");
    }
    assert_eq!(fake.requests(), before);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_write_updates_the_snapshot_without_rereading_s3() {
    let fake = crate::memory::s3::FakeS3::start();
    let root = temp();
    let service = counted_s3_service(&root, &fake);
    let repo = "org_writes_repo_0";
    let (user, label) = author();
    service
        .commit(repo, user, label, "first", &[write("note.md", "one")])
        .unwrap();
    service
        .commit(repo, user, label, "second", &[write("note.md", "two")])
        .unwrap();
    let (gets, _) = fake.requests();
    let store = service.read_store(repo).unwrap();
    assert_eq!(store.files[0].content, "two");
    assert_eq!(fake.requests().0, gets);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_lost_compare_and_swap_catches_up_and_both_writes_land() {
    let root = temp();
    let store = root.join("store");
    let server = |name: &str| {
        MemoryService::new(
            root.join(name),
            Backend::Directory(store.clone()),
            root.join(format!("scratch-{name}")),
            false,
        )
    };
    let (first, second) = (server("first"), server("second"));
    let repo = "org_swap_repo_0";
    let (user, label) = author();
    assert!(first.read_store(repo).unwrap().files.is_empty());
    assert!(second.read_store(repo).unwrap().files.is_empty());
    first
        .commit(repo, user, label, "first", &[write("a.md", "A")])
        .unwrap();
    let landed = second
        .commit(repo, user, label, "second", &[write("b.md", "B")])
        .unwrap();
    let paths: Vec<&str> = landed.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, vec!["a.md", "b.md"]);
    let third = server("third");
    let seen = third.read_store(repo).unwrap();
    let paths: Vec<&str> = seen.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, vec!["a.md", "b.md"]);
    assert_eq!(third.read_log(repo).unwrap().len(), 2);
    let _ = fs::remove_dir_all(root);
}

fn spawned() -> usize {
    super::git_cli_exec::SPAWNS.with(|count| count.get())
}

fn thousand_file_book(service: &MemoryService, repo: &str) {
    let (user, label) = author();
    let files: Vec<Change> = (0..1000)
        .map(|index| {
            write(
                &format!("notes/{}/{index}.md", index % 10),
                &format!("n{index}"),
            )
        })
        .collect();
    service.commit(repo, user, label, "seed", &files).unwrap();
}

#[test]
fn a_ui_commit_on_a_1000_file_book_runs_a_constant_number_of_git_processes() {
    let root = temp();
    let service = MemoryService::open_local(&root, false);
    let repo = "org_count_repo_0";
    let (user, label) = author();
    thousand_file_book(&service, repo);
    let before = spawned();
    let store = service
        .commit(
            repo,
            user,
            label,
            "edit one",
            &[write("notes/3/3.md", "changed")],
        )
        .unwrap();
    let used = spawned() - before;
    assert!(used < 15, "one edit spawned {used} git processes");
    assert_eq!(store.files.len(), 1000);
    let file = store.files.iter().find(|file| file.path == "notes/3/3.md");
    assert_eq!(file.unwrap().content, "changed");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn renames_deletes_and_folders_keep_the_snapshot_in_step_with_the_tree() {
    let root = temp();
    let service = MemoryService::open_local(&root, false);
    let repo = "org_edit_repo_0";
    let (user, label) = author();
    service
        .commit(
            repo,
            user,
            label,
            "seed",
            &[
                write("a.md", "A"),
                write("dir/.keep", ""),
                write("b.md", "B"),
            ],
        )
        .unwrap();
    let moved = service
        .commit(
            repo,
            user,
            label,
            "move",
            &[
                Change::Rename {
                    from: "a.md".into(),
                    to: "dir/a.md".into(),
                },
                Change::Delete {
                    path: "b.md".into(),
                },
                write("c.md", "A"),
            ],
        )
        .unwrap();
    let paths: Vec<&str> = moved.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, vec!["c.md", "dir/a.md"]);
    assert_eq!(moved.directories, vec!["dir".to_string()]);
    // A fresh process reading the repo sees the same book.
    let again = MemoryService::new(
        root.join("again"),
        Backend::Directory(root.join("store")),
        root.join("scratch-again"),
        false,
    );
    let seen = again.read_store(repo).unwrap();
    let seen: Vec<(&str, &str)> = seen
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.content.as_str()))
        .collect();
    assert_eq!(seen, vec![("c.md", "A"), ("dir/a.md", "A")]);
    let missing = service.commit(
        repo,
        user,
        label,
        "gone",
        &[Change::Delete {
            path: "nope.md".into(),
        }],
    );
    assert!(matches!(missing, Err(err) if err.message == "invalid memory path"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_push_of_three_commits_validates_with_a_constant_number_of_git_processes() {
    use std::process::Command;
    let root = temp();
    let service = MemoryService::open_local(&root, false);
    let repo = "org_push_repo_0";
    thousand_file_book(&service, repo);
    let bare = root.join("repos").join(format!("{repo}.git"));
    let work = root.join("work");
    let run = |dir: &Path, args: &[&str]| -> String {
        let out = Command::new("git")
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(["-c", "user.name=Ada", "-c", "user.email=ada@example.com"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    run(&root, &["clone", "-q", &bare.to_string_lossy(), "work"]);
    let old = run(&work, &["rev-parse", "HEAD"]);
    for index in 0..3 {
        fs::write(work.join(format!("pushed-{index}.md")), format!("p{index}")).unwrap();
        run(&work, &["add", "."]);
        let message = format!(
            "push {index}\n\nCadri-Author-Kind: human\nCadri-Author-Id: ada\nCadri-Author-Label: Ada\n"
        );
        run(&work, &["commit", "-q", "-m", &message]);
    }
    let new = run(&work, &["rev-parse", "HEAD"]);
    let pack = Command::new("git")
        .current_dir(&work)
        .args(["pack-objects", "--revs", "--stdout", "-q"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        let mut pack = pack;
        pack.stdin
            .take()
            .unwrap()
            .write_all(format!("{new}\n^{old}\n").as_bytes())
            .unwrap();
        let out = pack.wait_with_output().unwrap();
        let mut body = pkt(&format!("{old} {new} refs/heads/main\0report-status\n"));
        body.extend_from_slice(b"0000");
        body.extend(out.stdout);
        let before = spawned();
        let result = service
            .serve_git(repo, "git-receive-pack", true, None, &body, "ada")
            .unwrap();
        // index-pack is the one call that depends on the pack, not the book.
        let used = spawned() - before - 1;
        assert!(used < 15, "a push spawned {used} git processes");
        let super::protocol::GitHttp::Ready { body, .. } = result;
        assert!(String::from_utf8_lossy(&body).contains("ok refs/heads/main"));
    }
    let store = service.read_store(repo).unwrap();
    assert_eq!(store.files.len(), 1003);
    let _ = fs::remove_dir_all(root);
}

fn log_file(store: &Path, repo: &str, seq: u64) -> PathBuf {
    store
        .join("repos")
        .join(repo)
        .join(super::durable::log_key(seq))
}

fn log_header(store: &Path, repo: &str, seq: u64) -> super::bundle::BundleHeader {
    super::bundle::parse_bundle_header(&fs::read(log_file(store, repo, seq)).unwrap()).unwrap()
}

/// One state of a book written by the engine before the log format: the tip and
/// the full-history bundle of `refs/cadri/stage` stored under `objects/<tip>`.
struct Legacy {
    tip: String,
    bundle: Vec<u8>,
}

/// Commits each step in a scratch repo, the way the old engine did, and returns
/// the legacy layout after every step.
fn legacy_history(dir: &Path, steps: &[&[(&str, &str)]]) -> Vec<Legacy> {
    fs::create_dir_all(dir).unwrap();
    let run = |args: &[&str]| -> String {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["-c", "user.name=Ada", "-c", "user.email=ada@cadri.local"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    run(&["init", "-q", "-b", "main"]);
    let mut states = Vec::new();
    for (index, files) in steps.iter().enumerate() {
        for (path, content) in files.iter() {
            let target = dir.join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(target, content).unwrap();
        }
        run(&["add", "-A"]);
        let message = format!(
            "legacy {index}\n\nCadri-Author-Kind: human\nCadri-Author-Id: ada\nCadri-Author-Label: Ada\n"
        );
        run(&["commit", "-q", "-m", &message]);
        let tip = run(&["rev-parse", "HEAD"]);
        run(&["update-ref", "refs/cadri/stage", &tip]);
        let path = dir.join(".git").join(format!("{tip}.bundle"));
        run(&[
            "bundle",
            "create",
            "-q",
            &path.to_string_lossy(),
            "refs/cadri/stage",
        ]);
        run(&["update-ref", "-d", "refs/cadri/stage"]);
        states.push(Legacy {
            tip,
            bundle: fs::read(&path).unwrap(),
        });
    }
    states
}

/// Writes the legacy keys for `repo` under a directory store root.
fn legacy_layout(root: &Path, repo: &str, legacy: &Legacy, with_bundle: bool) {
    let book = root.join("repos").join(repo);
    fs::create_dir_all(book.join("refs/heads")).unwrap();
    fs::write(book.join("refs/heads/main"), format!("{}\n", legacy.tip)).unwrap();
    if with_bundle {
        fs::create_dir_all(book.join("objects")).unwrap();
        fs::write(book.join("objects").join(&legacy.tip), &legacy.bundle).unwrap();
    }
}

fn legacy_s3(books: &S3Books, repo: &str, legacy: &Legacy, with_bundle: bool) {
    let tip = format!("{}\n", legacy.tip);
    assert!(books
        .create_key(repo, "refs/heads/main", tip.as_bytes())
        .unwrap());
    if with_bundle {
        let key = format!("objects/{}", legacy.tip);
        assert!(books.create_key(repo, &key, &legacy.bundle).unwrap());
    }
}

fn file_pairs(store: &crate::memory::types::MemoryStore) -> Vec<(String, String)> {
    store
        .files
        .iter()
        .map(|file| (file.path.clone(), file.content.clone()))
        .collect()
}

#[test]
fn the_first_write_creates_log_entry_one_without_prerequisites() {
    let root = temp();
    let service = MemoryService::open_local(&root, false);
    let repo = "org_log_repo_0";
    let (user, label) = author();
    service
        .commit(repo, user, label, "first", &[write("a.md", "A")])
        .unwrap();
    let tip = service.read_log(repo).unwrap()[0].id.clone();
    let header = log_header(&root.join("store"), repo, 1);
    assert!(header.prerequisites.is_empty());
    assert_eq!(header.refs, [(tip.clone(), "refs/cadri/stage".to_string())]);
    assert_eq!(service.read_tip(repo).unwrap(), (Some(tip), 1));
    let size = fs::metadata(log_file(&root.join("store"), repo, 1))
        .unwrap()
        .len();
    assert_eq!(service.writer_state(repo), (1, 0, size));
    let book = root.join("store/repos").join(repo);
    assert!(!book.join("refs").exists());
    assert!(!book.join("objects").exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn the_second_write_creates_log_entry_two_whose_prerequisite_is_the_first_tip() {
    let root = temp();
    let service = MemoryService::open_local(&root, false);
    let repo = "org_log_repo_0";
    let (user, label) = author();
    service
        .commit(repo, user, label, "first", &[write("a.md", "A")])
        .unwrap();
    service
        .commit(repo, user, label, "second", &[write("b.md", "B")])
        .unwrap();
    let log = service.read_log(repo).unwrap();
    let (second, first) = (&log[0].id, &log[1].id);
    let header = log_header(&root.join("store"), repo, 2);
    assert_eq!(header.prerequisites, std::slice::from_ref(first));
    assert_eq!(
        header.refs,
        [(second.clone(), "refs/cadri/stage".to_string())]
    );
    assert_eq!(service.read_tip(repo).unwrap().1, 2);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_one_line_edit_on_a_1000_file_book_stores_less_than_16_kib() {
    let root = temp();
    let service = MemoryService::open_local(&root, false);
    let repo = "org_size_repo_0";
    let (user, label) = author();
    let body: String = (0..40)
        .map(|line| format!("line {line} of a note\n"))
        .collect();
    let files: Vec<Change> = (0..1000)
        .map(|index| write(&format!("notes/{}/{index}.md", index % 20), &body))
        .collect();
    service.commit(repo, user, label, "seed", &files).unwrap();
    let edited = body.replacen("line 7 of a note", "line 7 was edited", 1);
    service
        .commit(
            repo,
            user,
            label,
            "one line",
            &[write("notes/3/3.md", &edited)],
        )
        .unwrap();
    let size = fs::metadata(log_file(&root.join("store"), repo, 2))
        .unwrap()
        .len();
    println!("bench log_entry_one_line_edit_1000_files_bytes={size}");
    assert!(size < 16 * 1024, "a one-line edit stored {size} bytes");
    let _ = fs::remove_dir_all(root);
}

fn assert_restart_loads_the_same_book(first: &MemoryService, fresh: &MemoryService, repo: &str) {
    let (user, label) = author();
    first
        .commit(
            repo,
            user,
            label,
            "seed",
            &[
                write("a.md", "A"),
                write("b.md", "B"),
                write("dir/.keep", ""),
            ],
        )
        .unwrap();
    first
        .commit(
            repo,
            user,
            label,
            "move",
            &[
                Change::Rename {
                    from: "a.md".into(),
                    to: "dir/a.md".into(),
                },
                write("b.md", "B2"),
            ],
        )
        .unwrap();
    first
        .commit(repo, user, label, "third", &[write("c.md", "C")])
        .unwrap();
    let before = first.read_store(repo).unwrap();
    let loaded = fresh.read_store(repo).unwrap();
    assert_eq!(file_pairs(&loaded), file_pairs(&before));
    assert_eq!(loaded.directories, before.directories);
    assert_eq!(fresh.read_tip(repo).unwrap(), first.read_tip(repo).unwrap());
    assert_eq!(fresh.read_tip(repo).unwrap().1, 3);
    let ids = |service: &MemoryService| -> Vec<String> {
        service
            .read_log(repo)
            .unwrap()
            .into_iter()
            .map(|commit| commit.id)
            .collect()
    };
    assert_eq!(ids(fresh), ids(first));
    assert_eq!(fresh.writer_state(repo).0, 3);
    // The restarted service keeps appending where the first one stopped.
    fresh
        .commit(repo, user, label, "after restart", &[write("d.md", "D")])
        .unwrap();
    assert_eq!(fresh.read_tip(repo).unwrap().1, 4);
}

#[test]
fn a_restarted_service_loads_the_same_tip_and_files() {
    let root = temp();
    let repo = "org_restart_repo_0";
    let first = MemoryService::open_local(root.join("a"), false);
    let fresh = MemoryService::open_at(
        root.join("b/repos"),
        root.join("a/store"),
        root.join("b/scratch"),
    );
    assert_restart_loads_the_same_book(&first, &fresh, repo);

    let fake = crate::memory::s3::FakeS3::start();
    let first = counted_s3_service(&root.join("s3-a"), &fake);
    let fresh = counted_s3_service(&root.join("s3-b"), &fake);
    assert_restart_loads_the_same_book(&first, &fresh, repo);
    let _ = fs::remove_dir_all(root);
}

fn assert_legacy_migrates(
    service: &dyn Fn() -> MemoryService,
    stored: &dyn Fn(&str) -> Option<Vec<u8>>,
    history: &[Legacy],
    repo: &str,
) {
    let legacy = history.last().unwrap();
    let first = service();
    let files = first.read_store(repo).unwrap();
    assert_eq!(
        file_pairs(&files),
        [
            ("a.md".to_string(), "A2".to_string()),
            ("b.md".to_string(), "B".to_string()),
        ]
    );
    let log = first.read_log(repo).unwrap();
    let ids: Vec<&str> = log.iter().map(|commit| commit.id.as_str()).collect();
    assert_eq!(ids, [legacy.tip.as_str(), history[0].tip.as_str()]);
    assert_eq!(log[1].message, "legacy 0");
    assert_eq!(first.read_tip(repo).unwrap(), (Some(legacy.tip.clone()), 0));
    assert_eq!(first.writer_state(repo), (0, 0, 0));
    assert_eq!(
        stored("checkpoint/00000000000000000000").as_deref(),
        Some(legacy.bundle.as_slice())
    );

    let (user, label) = author();
    first
        .commit(repo, user, label, "after migration", &[write("c.md", "C")])
        .unwrap();
    let entry = stored("log/00000000000000000001").unwrap();
    let header = super::bundle::parse_bundle_header(&entry).unwrap();
    assert_eq!(header.prerequisites, std::slice::from_ref(&legacy.tip));
    assert_eq!(first.writer_state(repo).0, 1);

    let restarted = service();
    let files = restarted.read_store(repo).unwrap();
    assert_eq!(files.files.len(), 3);
    assert_eq!(restarted.read_log(repo).unwrap().len(), 3);
    assert_eq!(
        restarted.read_tip(repo).unwrap(),
        first.read_tip(repo).unwrap()
    );
    assert_eq!(restarted.writer_state(repo), (1, 0, entry.len() as u64));
    // Legacy keys are left as they were.
    assert_eq!(
        stored("refs/heads/main"),
        Some(format!("{}\n", legacy.tip).into_bytes())
    );
    assert_eq!(
        stored(&format!("objects/{}", legacy.tip)),
        Some(legacy.bundle.clone())
    );
}

#[test]
fn a_legacy_book_migrates_to_checkpoint_zero_and_keeps_its_history() {
    let root = temp();
    let history = legacy_history(
        &root.join("legacy"),
        &[&[("a.md", "A"), ("b.md", "B")], &[("a.md", "A2")]],
    );
    let repo = "org_legacy_repo_0";
    let store = root.join("store");
    legacy_layout(&store, repo, history.last().unwrap(), true);
    let next = std::cell::Cell::new(0);
    let service = || {
        next.set(next.get() + 1);
        let name = format!("server-{}", next.get());
        MemoryService::open_at(
            root.join(&name).join("repos"),
            store.clone(),
            root.join(&name).join("scratch"),
        )
    };
    let stored = |key: &str| fs::read(store.join("repos").join(repo).join(key)).ok();
    assert_legacy_migrates(&service, &stored, &history, repo);

    let fake = crate::memory::s3::FakeS3::start();
    let books = S3Books::from_client("memories", S3Books::test_client(&fake.endpoint));
    legacy_s3(&books, repo, history.last().unwrap(), true);
    let service = || {
        next.set(next.get() + 1);
        counted_s3_service(&root.join(format!("s3-{}", next.get())), &fake)
    };
    let stored = |key: &str| books.get_key(repo, key).unwrap();
    assert_legacy_migrates(&service, &stored, &history, repo);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_lost_log_create_catches_up_and_the_ui_commit_retries() {
    let root = temp();
    let store = root.join("store");
    let server = |name: &str| {
        MemoryService::open_at(
            root.join(name).join("repos"),
            store.clone(),
            root.join(name).join("scratch"),
        )
    };
    let (first, second) = (server("first"), server("second"));
    let repo = "org_lost_repo_0";
    let (user, label) = author();
    first
        .commit(repo, user, label, "seed", &[write("seed.md", "S")])
        .unwrap();
    assert_eq!(second.read_store(repo).unwrap().files.len(), 1);
    first
        .commit(repo, user, label, "first", &[write("a.md", "A")])
        .unwrap();
    // `second` still believes log entry 2 is free.
    let landed = second
        .commit(repo, user, label, "second", &[write("b.md", "B")])
        .unwrap();
    let paths: Vec<&str> = landed.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, vec!["a.md", "b.md", "seed.md"]);
    assert_eq!(second.read_tip(repo).unwrap().1, 3);
    assert_eq!(second.writer_state(repo).0, 3);
    let log = second.read_log(repo).unwrap();
    assert_eq!(log.len(), 3);
    assert_eq!(log[0].message, "second");
    let header = log_header(&store, repo, 3);
    assert_eq!(header.prerequisites, [log[1].id.clone()]);
    let third = server("third");
    assert_eq!(
        file_pairs(&third.read_store(repo).unwrap()),
        file_pairs(&landed)
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_log_gap_is_reported_not_repaired() {
    let root = temp();
    let service = MemoryService::open_local(root.join("a"), false);
    let repo = "org_gap_repo_0";
    let (user, label) = author();
    for (index, name) in ["a.md", "b.md", "c.md"].iter().enumerate() {
        service
            .commit(
                repo,
                user,
                label,
                &format!("write {index}"),
                &[write(name, "x")],
            )
            .unwrap();
    }
    let store = root.join("a/store");
    fs::remove_file(log_file(&store, repo, 2)).unwrap();
    let fresh = MemoryService::open_at(root.join("b/repos"), store.clone(), root.join("b/scratch"));
    let err = fresh.read_store(repo).unwrap_err();
    assert_eq!(err.status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(err.message, "memory log gap after 1");
    // Nothing was rewritten to paper over the gap.
    assert!(!log_file(&store, repo, 2).exists());
    assert!(log_file(&store, repo, 3).exists());
    let _ = fs::remove_dir_all(root);
}

fn edit_request(message: &str, changes: Vec<Change>) -> crate::memory::types::EditRequest {
    let (user, label) = author();
    crate::memory::types::EditRequest {
        user_id: user.to_string(),
        label: label.to_string(),
        message: message.to_string(),
        changes,
        base: None,
        agent: None,
    }
}

fn tip_of(service: &MemoryService, repo: &str) -> String {
    service.read_tip(repo).unwrap().0.unwrap()
}

#[test]
fn twenty_threads_of_five_edits_all_land_in_fewer_log_entries_than_commits() {
    let fake = crate::memory::s3::FakeS3::start();
    fake.set_latency(std::time::Duration::from_millis(20));
    let root = temp();
    let service = counted_s3_service(&root, &fake);
    let repo = "org_group_repo_0";
    service.read_store(repo).unwrap();
    let handles: Vec<_> = (0..20)
        .map(|thread| {
            let service = service.clone();
            std::thread::spawn(move || {
                for edit in 0..5 {
                    let mut request = edit_request(
                        &format!("thread {thread} edit {edit}"),
                        vec![write(&format!("t{thread}/e{edit}.md"), "x\n")],
                    );
                    if thread % 2 == 1 {
                        request.agent = Some(format!("agent-{thread}"));
                    }
                    let outcome = service.edit(repo, request).unwrap();
                    assert!(outcome.commit.is_some());
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let log = service.read_log(repo).unwrap();
    assert_eq!(log.len(), 100);
    for commit in &log {
        let thread: usize = commit.message["thread ".len()..]
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let expected = if thread % 2 == 1 {
            Author::Robot {
                id: format!("agent-{thread}"),
            }
        } else {
            Author::Human {
                user_id: "ada".to_string(),
                label: "Ada".to_string(),
            }
        };
        assert_eq!(commit.author, expected, "{}", commit.message);
    }
    assert_eq!(service.read_store(repo).unwrap().files.len(), 100);
    let entries = service.read_tip(repo).unwrap().1;
    assert!((1..100).contains(&entries), "{entries} log entries");
    let fresh = counted_s3_service(&temp(), &fake);
    assert_eq!(
        fresh.read_tip(repo).unwrap(),
        service.read_tip(repo).unwrap()
    );
    assert_eq!(fresh.read_store(repo).unwrap().files.len(), 100);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn an_edit_with_a_stale_base_touching_a_changed_file_is_a_conflict() {
    let root = temp();
    let service = MemoryService::open_local(&root, false);
    let repo = "org_base0_repo_0";
    service
        .edit(
            repo,
            edit_request("seed", vec![write("a.md", "1"), write("b.md", "1")]),
        )
        .unwrap();
    let base = tip_of(&service, repo);
    service
        .edit(repo, edit_request("teammate", vec![write("a.md", "2")]))
        .unwrap();
    let tip = tip_of(&service, repo);
    let mut stale = edit_request(
        "mine",
        vec![
            write("a.md", "3"),
            Change::Rename {
                from: "b.md".to_string(),
                to: "c.md".to_string(),
            },
        ],
    );
    stale.base = Some(base);
    let err = service.edit(repo, stale).err().unwrap();
    assert_eq!(err.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(err.message, "conflict");
    assert_eq!(err.paths, Some(vec!["a.md".to_string()]));
    assert_eq!(tip_of(&service, repo), tip);

    let mut unknown = edit_request("mine", vec![write("d.md", "4")]);
    unknown.base = Some("1".repeat(40));
    let err = service.edit(repo, unknown).err().unwrap();
    assert_eq!(err.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(err.message, "unknown base");
    assert_eq!(err.paths, None);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn an_edit_with_a_stale_base_touching_other_files_lands() {
    let root = temp();
    let service = MemoryService::open_local(&root, false);
    let repo = "org_base1_repo_0";
    service
        .edit(
            repo,
            edit_request("seed", vec![write("a.md", "1"), write("b.md", "1")]),
        )
        .unwrap();
    let base = tip_of(&service, repo);
    service
        .edit(repo, edit_request("teammate", vec![write("a.md", "2")]))
        .unwrap();
    let mut stale = edit_request("mine", vec![write("b.md", "3")]);
    stale.base = Some(base);
    let outcome = service.edit(repo, stale).unwrap();
    let commit = outcome.commit.clone().unwrap();
    assert_eq!(tip_of(&service, repo), commit);
    let tree = outcome.tree();
    assert_eq!(tree.commit.as_deref(), Some(commit.as_str()));
    assert_eq!(tree.tree.tip.as_deref(), Some(commit.as_str()));
    assert_eq!(
        file_pairs(&service.read_store(repo).unwrap()),
        [
            ("a.md".to_string(), "2".to_string()),
            ("b.md".to_string(), "3".to_string())
        ]
    );
    let mut current = edit_request("again", vec![write("a.md", "4")]);
    current.base = Some(commit);
    service.edit(repo, current).unwrap();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn edits_without_a_base_are_last_writer_wins() {
    let root = temp();
    let service = MemoryService::open_local(&root, false);
    let repo = "org_base2_repo_0";
    service
        .edit(repo, edit_request("first", vec![write("a.md", "1")]))
        .unwrap();
    service
        .edit(repo, edit_request("second", vec![write("a.md", "2")]))
        .unwrap();
    assert_eq!(service.read_store(repo).unwrap().files[0].content, "2");
    let noop = service
        .edit(repo, edit_request("same", vec![write("a.md", "2")]))
        .unwrap();
    assert_eq!(noop.commit, None);
    assert_eq!(service.read_log(repo).unwrap().len(), 2);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn an_agent_edit_is_authored_as_a_robot() {
    let root = temp();
    let service = MemoryService::open_local(&root, false);
    let repo = "org_robot0_repo_0";
    let mut request = edit_request("agent note", vec![write("a.md", "1")]);
    request.agent = Some("claude-code-7f3a".to_string());
    let commit = service.edit(repo, request).unwrap().commit.unwrap();
    let log = service.read_log(repo).unwrap();
    assert_eq!(
        log[0].author,
        Author::Robot {
            id: "claude-code-7f3a".to_string()
        }
    );
    assert_eq!(log[0].message, "agent note");
    let git = super::git_cli_exec::Repo {
        git_dir: root.join("repos").join(format!("{repo}.git")),
        scratch: root.join("scratch").join(repo),
        jail: false,
    };
    let raw = super::git_cli_exec::git_text(
        &git,
        &["log", "-1", "--format=%an <%ae>%n%B", &commit],
        None,
        &[],
    )
    .unwrap();
    assert_eq!(
        raw,
        "claude-code-7f3a <claude-code-7f3a@agents.cadri.local>\nagent note\n\n\
         Cadri-Author-Kind: robot\nCadri-Author-Id: claude-code-7f3a\nCadri-Author-Label: Ada"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn an_invalid_agent_id_is_rejected() {
    let root = temp();
    let service = MemoryService::open_local(&root, false);
    let repo = "org_robot1_repo_0";
    let long = "a".repeat(65);
    for agent in ["", "has space", "new\nline", "slash/agent", long.as_str()] {
        let mut request = edit_request("agent note", vec![write("a.md", "1")]);
        request.agent = Some(agent.to_string());
        let err = service.edit(repo, request).err().unwrap();
        assert_eq!(err.status, axum::http::StatusCode::BAD_REQUEST, "{agent:?}");
        assert_eq!(err.message, "invalid agent");
    }
    let mut request = edit_request("agent note", vec![write("a.md", "1")]);
    request.agent = Some("a".repeat(64));
    service.edit(repo, request).unwrap();
    assert_eq!(service.read_log(repo).unwrap().len(), 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn one_bad_edit_in_a_batch_does_not_fail_the_others() {
    let root = temp();
    let service = MemoryService::open_local(&root, false);
    let repo = "org_batch0_repo_0";
    service.read_store(repo).unwrap();
    // Hold the writer so every edit below queues up and runs as one batch.
    let entered = std::sync::atomic::AtomicBool::new(false);
    let results = std::thread::scope(|scope| {
        let holder = scope
            .spawn(|| service.hold_writer(repo, std::time::Duration::from_millis(500), &entered));
        while !entered.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::yield_now();
        }
        let edits: Vec<_> = ["a.md", "../escape.md", "b.md"]
            .into_iter()
            .map(|path| {
                let service = &service;
                scope.spawn(move || service.edit(repo, edit_request(path, vec![write(path, "x")])))
            })
            .collect();
        holder.join().unwrap();
        edits
            .into_iter()
            .map(|edit| edit.join().unwrap())
            .collect::<Vec<_>>()
    });
    let err = results[1].as_ref().err().unwrap();
    assert_eq!(err.status, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(err.message, "invalid memory path");
    assert!(results[0].as_ref().unwrap().commit.is_some());
    assert!(results[2].as_ref().unwrap().commit.is_some());
    let paths: Vec<String> = service
        .read_store(repo)
        .unwrap()
        .files
        .into_iter()
        .map(|file| file.path)
        .collect();
    assert_eq!(paths, ["a.md", "b.md"]);
    assert_eq!(service.read_log(repo).unwrap().len(), 2);
    assert_eq!(service.read_tip(repo).unwrap().1, 1);
    let _ = fs::remove_dir_all(root);
}
