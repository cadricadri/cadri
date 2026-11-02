//! Unit tests for the memory git service.

use std::fs;
use std::path::{Path, PathBuf};

use crate::memory::types::{Author, Change};
use crate::memory::s3::S3Books;

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
    assert_eq!(
        saved.files[0].blame[0].author,
        Author::Human {
            user_id: "ada".into(),
            label: "Ada".into(),
        }
    );
    assert_eq!(saved.commits[0].message, "Add a note");
    assert_eq!(saved.commits.len(), 1);
    let again = service
        .commit(
            repo,
            user,
            label,
            "Add a note",
            &[write("notes.md", "hello")],
        )
        .unwrap();
    assert_eq!(again.commits.len(), 1);

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
    assert_eq!(wide.commits[0].message, "many notes");

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
        std::thread::spawn(move || {
            left.commit(repo, user, label, "left", &[write("left.md", "L")])
        })
    };
    let right_commit = std::thread::spawn(move || {
        right.commit(repo, user, label, "right", &[write("right.md", "R")])
    });
    left_commit.join().unwrap().unwrap();
    right_commit.join().unwrap().unwrap();
    let store = left.read_store(repo).unwrap();
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
    let fresh = service.read_store(repo).unwrap();
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
    let from_mount = service.read_store(repo).unwrap();
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
    let from_mount = reader.read_store(repo).unwrap();
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
    let saved = service
        .commit(
            repo,
            user,
            label,
            "Add a note",
            &[write("note.md", "hello")],
        )
        .unwrap();
    let sha = saved.commits[0].id.clone();
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
