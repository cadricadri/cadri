//! Unit tests for the memory git service.

use std::fs;
use std::path::{Path, PathBuf};

use crate::memory::s3::S3Books;
use crate::memory::types::{Author, Change};

use super::error::Fail;
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
fn bundle_put_without_cas_leaves_the_ref_unchanged() {
    let root = temp();
    let service = MemoryService::open_local(&root, None, false);
    let repo = "org_test_repo_0";
    service
        .put_bundle(repo, &"a".repeat(40), b"bundle")
        .unwrap();
    assert_eq!(service.read_ref(repo).unwrap(), None);
    service.cas_ref(repo, None, &"a".repeat(40)).unwrap();
    assert_eq!(service.read_ref(repo).unwrap().unwrap(), "a".repeat(40));
    assert!(matches!(
        service.cas_ref(repo, None, &"b".repeat(40)),
        Err(Fail::Conflict)
    ));
    service
        .cas_ref(repo, Some(&"a".repeat(40)), &"b".repeat(40))
        .unwrap();
    assert!(matches!(
        service.cas_ref(repo, Some(&"a".repeat(40)), &"c".repeat(40)),
        Err(Fail::Conflict)
    ));
    assert_eq!(service.read_ref(repo).unwrap().unwrap(), "b".repeat(40));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn ui_commit_round_trip_stores_a_large_book() {
    let root = temp();
    let service = MemoryService::open_local(&root, None, false);
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
        None,
        false,
    );
    let right = MemoryService::new(
        root.join("right"),
        Backend::Directory(store),
        root.join("sr"),
        None,
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
        None,
        false,
    );
    let store = fresh.read_store(repo).unwrap();
    let paths: Vec<&str> = store.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, vec!["left.md", "right.md"]);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn stale_mount_does_not_hide_the_published_tip() {
    let root = temp();
    let mount = root.join("mount");
    fs::create_dir_all(&mount).unwrap();
    let service = MemoryService::open_local(root.join("data"), Some(mount.clone()), false);
    let repo = "org_cache_repo_0";
    let (user, label) = author();
    service
        .commit(repo, user, label, "first", &[write("note.md", "one")])
        .unwrap();
    let store_dir = root.join("data/store");
    copy_dir(&store_dir, &mount);
    service
        .commit(repo, user, label, "second", &[write("note.md", "two")])
        .unwrap();
    let git_dir = root.join("data/repos").join(format!("{repo}.git"));
    fs::remove_dir_all(&git_dir).unwrap();
    let restarted = MemoryService::open_local(root.join("data"), Some(mount.clone()), false);
    let fresh = restarted.read_store(repo).unwrap();
    assert_eq!(fresh.files[0].content, "two");

    copy_dir(&store_dir, &mount);
    let object = store_dir.join("repos").join(repo).join("objects");
    for entry in fs::read_dir(&object).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            fs::remove_file(path).unwrap();
        }
    }
    fs::remove_dir_all(&git_dir).ok();
    let restarted = MemoryService::open_local(root.join("data"), Some(mount.clone()), false);
    let from_mount = restarted.read_store(repo).unwrap();
    assert_eq!(from_mount.files[0].content, "two");
    let _ = fs::remove_dir_all(root);
}

fn copy_dir(from: &Path, to: &Path) {
    let _ = fs::remove_dir_all(to);
    fs::create_dir_all(to).unwrap();
    for entry in walkdir_files(from) {
        let rel = entry.strip_prefix(from).unwrap();
        let dest = to.join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::copy(&entry, dest).unwrap();
    }
}

fn walkdir_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(&current).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files
}

#[test]
fn s3_cas_then_a_second_cache_reads_the_tip() {
    let fake = crate::memory::s3::FakeS3::start();
    let client = S3Books::test_client(&fake.endpoint);
    let root = temp();
    let repo = "org_test_repo_0";
    let sha_a = "a".repeat(40);
    let sha_b = "b".repeat(40);
    let left = MemoryService::open_s3(
        root.join("left"),
        None,
        false,
        S3Books::from_client("memories", client.clone()),
    );
    left.put_bundle(repo, &sha_a, b"bundle-a").unwrap();
    assert_eq!(left.read_ref(repo).unwrap(), None);
    left.cas_ref(repo, None, &sha_a).unwrap();
    assert!(matches!(
        left.cas_ref(repo, None, &sha_b),
        Err(Fail::Conflict)
    ));
    left.cas_ref(repo, Some(&sha_a), &sha_b).unwrap();
    assert!(matches!(
        left.cas_ref(repo, Some(&sha_a), &"c".repeat(40)),
        Err(Fail::Conflict)
    ));
    let right = MemoryService::open_s3(
        root.join("right"),
        None,
        false,
        S3Books::from_client("memories", client),
    );
    assert_eq!(right.read_ref(repo).unwrap().unwrap(), sha_b);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn s3_commit_is_read_by_another_cache_and_ignores_a_stale_mount() {
    let fake = crate::memory::s3::FakeS3::start();
    let client = S3Books::test_client(&fake.endpoint);
    let root = temp();
    let mount = root.join("mount");
    fs::create_dir_all(&mount).unwrap();
    let repo = "org_cache_repo_0";
    let (user, label) = author();
    let writer = MemoryService::open_s3(
        root.join("writer"),
        Some(mount.clone()),
        false,
        S3Books::from_client("memories", client.clone()),
    );
    writer
        .commit(repo, user, label, "first", &[write("note.md", "one")])
        .unwrap();
    let first = writer.read_ref(repo).unwrap().unwrap();
    let bundle = writer.load_bundle(repo, &first).unwrap();
    let mount_repo = mount.join("repos").join(repo);
    fs::create_dir_all(mount_repo.join("objects")).unwrap();
    fs::create_dir_all(mount_repo.join("refs/heads")).unwrap();
    fs::write(mount_repo.join("refs/heads/main"), format!("{first}\n")).unwrap();
    fs::write(mount_repo.join("objects").join(&first), &bundle).unwrap();

    writer
        .commit(repo, user, label, "second", &[write("note.md", "two")])
        .unwrap();
    let second = writer.read_ref(repo).unwrap().unwrap();
    assert_ne!(first, second);

    let reader = MemoryService::open_s3(
        root.join("reader"),
        Some(mount.clone()),
        false,
        S3Books::from_client("memories", client),
    );
    let fresh = reader.read_store(repo).unwrap();
    assert_eq!(fresh.files[0].content, "two");

    let second_bundle = reader.load_bundle(repo, &second).unwrap();
    fs::write(mount_repo.join("refs/heads/main"), format!("{second}\n")).unwrap();
    fs::write(mount_repo.join("objects").join(&second), &second_bundle).unwrap();
    fake.remove(&format!("repos/{repo}/objects/{second}"));
    fs::remove_dir_all(root.join("reader/repos")).ok();
    let restarted = MemoryService::open_s3(
        root.join("reader"),
        Some(mount.clone()),
        false,
        S3Books::from_client("memories", S3Books::test_client(&fake.endpoint)),
    );
    let from_mount = restarted.read_store(repo).unwrap();
    assert_eq!(from_mount.files[0].content, "two");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn ui_commit_returns_conflict_after_eight_lost_swaps() {
    let root = temp();
    let service = MemoryService::open_local(&root, None, false);
    let repo = "org_test_repo_0";
    let (user, label) = author();
    service.fail_next_cas(8);
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
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_failed_bundle_put_removes_the_staging_ref() {
    let fake = crate::memory::s3::FakeS3::start();
    fake.fail_next_object_put();
    let root = temp();
    let service = MemoryService::open_s3(
        &root,
        None,
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
    let service = MemoryService::open_local(&root, None, false);
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
    let stored = fs::read(
        root.join("store/repos")
            .join(repo)
            .join("objects")
            .join(&sha),
    )
    .unwrap();
    let first = rebuild_bundle(&git_dir, &sha);
    let second = rebuild_bundle(&git_dir, &sha);
    assert_eq!(stored, first);
    assert_eq!(first, second);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn s3_client_quotes_the_etag_and_skips_checksums() {
    let fake = crate::memory::s3::FakeS3::start();
    let root = temp();
    let service = MemoryService::open_s3(
        &root,
        None,
        false,
        S3Books::from_client("memories", S3Books::test_client(&fake.endpoint)),
    );
    let repo = "org_cache_repo_0";
    let (user, label) = author();
    service
        .commit(repo, user, label, "first", &[write("note.md", "one")])
        .unwrap();
    service
        .commit(repo, user, label, "second", &[write("note.md", "two")])
        .unwrap();
    let tag = fake.last_ref_match().expect("ref update sent If-Match");
    assert!(
        tag.starts_with('"') && tag.ends_with('"') && tag.len() == 34,
        "{tag}"
    );
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
    let service = MemoryService::open_local(&root, None, false);
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
    let service = MemoryService::open_local(&root, None, false);
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
    let service = MemoryService::open_local(&root, None, false);
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
        None,
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
            None,
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
    let service = MemoryService::open_local(&root, None, false);
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
    let service = MemoryService::open_local(&root, None, false);
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
        None,
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
    let service = MemoryService::open_local(&root, None, false);
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
