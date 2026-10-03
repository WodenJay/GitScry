mod support;

use std::{
    env, fs,
    fs::OpenOptions,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use rusqlite::{Connection, params};
use support::{TestRepo, git, git_command, git_stdout};

fn commit(repo: &TestRepo, contents: &str, subject: &str) {
    fs::write(repo.dir.path().join("history.txt"), contents).expect("write history");
    git(repo.dir.path(), ["add", "--all"]);
    let output = git_command(repo.dir.path())
        .args(["commit", "-m", subject])
        .output()
        .expect("commit history");
    assert!(
        output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn add_linked_worktree(repo: &TestRepo) -> (tempfile::TempDir, PathBuf) {
    let parent = tempfile::tempdir().expect("create worktree parent");
    let path = parent.path().join("linked");
    git(
        repo.dir.path(),
        [
            "worktree",
            "add",
            "--detach",
            path.to_str().expect("worktree path is UTF-8"),
            "HEAD",
        ],
    );
    (parent, path)
}

fn cache_snapshot(cache: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = fs::read_dir(cache)
        .expect("read cache directory")
        .map(|entry| {
            let entry = entry.expect("read cache entry");
            (
                entry.file_name().to_string_lossy().into_owned(),
                fs::read(entry.path()).expect("read cache file"),
            )
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

fn add_semantic_vector(repo: &TestRepo) {
    let connection = Connection::open(repo.cache_dir().join("cache.sqlite")).unwrap();
    let fingerprint = "f".repeat(64);
    connection
        .execute(
            "INSERT INTO semantic_vectors
                (commit_id, commit_oid, embedding, source_fingerprint,
                 input_fingerprint, encoder_fingerprint, runtime_provenance)
             SELECT commit_id, oid, zeroblob(1536), ?1, ?1, ?1, 'test fixture'
             FROM commits ORDER BY commit_id LIMIT 1",
            params![fingerprint],
        )
        .unwrap();
}

fn data_file_bytes(files: &[(String, Vec<u8>)]) -> usize {
    files
        .iter()
        .filter(|(name, _)| name != "cache.lock" && name != ".gitignore")
        .map(|(_, contents)| contents.len())
        .sum()
}

#[test]
fn clear_dry_run_reports_and_preserves_repository_cache() {
    let repo = TestRepo::new();
    commit(&repo, "first version\n", "Initial clear preview history");
    commit(&repo, "second version\n", "Update clear preview history");
    repo.index();
    add_semantic_vector(&repo);

    let ignore_file = repo.cache_dir().join(".gitignore");
    let ignore_contents = b"# clear preview must preserve this file\n";
    fs::write(&ignore_file, ignore_contents).unwrap();
    let previous = repo.cache_dir().join("cache.sqlite.previous");
    let staging = repo.cache_dir().join("cache.sqlite.staging-abandoned");
    fs::write(&previous, b"previous generation").unwrap();
    fs::write(&staging, b"abandoned staging data").unwrap();
    let marker = repo.user_data_dir().join("shared-model-resource-marker");
    fs::write(&marker, b"must survive clear").unwrap();

    let before = cache_snapshot(&repo.cache_dir());
    let before_bytes = data_file_bytes(&before);
    let head = repo.head();
    let refs = git_stdout(repo.dir.path(), ["show-ref"]);
    let status = git_stdout(repo.dir.path(), ["status", "--porcelain"]);
    let (_worktree_parent, linked) = add_linked_worktree(&repo);
    let output =
        TestRepo::run_at_with_user_data_dir(&linked, ["clear", "--dry-run"], repo.user_data_dir());
    assert_eq!(
        output.status.code(),
        Some(0),
        "clear --dry-run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    for name in [
        "cache.sqlite",
        "cache.sqlite.previous",
        "cache.sqlite.staging-abandoned",
    ] {
        assert!(stdout.contains(name), "missing {name}:\n{stdout}");
    }
    for count in [
        "commits: 2",
        "changes: 2",
        "path records: 2",
        "hunks: 2",
        "semantic vectors: 1",
    ] {
        assert!(stdout.contains(count), "missing {count}:\n{stdout}");
    }
    assert!(stdout.contains(&format!("before: {before_bytes} bytes")));
    assert!(stdout.contains("projected after: 0 bytes"));
    assert!(stdout.contains(&format!("would release: {before_bytes} bytes")));
    assert!(stdout.contains("Retained: gitscry/cache.lock"));
    assert!(!stdout.contains("  gitscry/cache.lock ("));
    assert!(!stdout.contains("  gitscry/.gitignore ("));

    assert_eq!(fs::read(&ignore_file).unwrap(), ignore_contents);
    assert_eq!(cache_snapshot(&repo.cache_dir()), before);
    assert_eq!(repo.head(), head);
    assert_eq!(git_stdout(repo.dir.path(), ["show-ref"]), refs);
    assert_eq!(
        git_stdout(repo.dir.path(), ["status", "--porcelain"]),
        status
    );
    assert_eq!(fs::read(marker).unwrap(), b"must survive clear");
}

#[test]
fn clear_from_linked_worktree_preserves_git_and_reindex_works() {
    let repo = TestRepo::new();
    commit(
        &repo,
        "reindex marker\n",
        "Clear reindex preservation marker",
    );
    repo.index();
    let (worktree_parent, linked) = add_linked_worktree(&repo);
    let cache_lock = repo.cache_dir().join("cache.lock");
    assert!(cache_lock.is_file());
    let head = repo.head();
    let refs = git_stdout(repo.dir.path(), ["show-ref"]);
    let marker = repo.user_data_dir().join("shared-model-resource-marker");
    fs::write(&marker, b"must survive clear").unwrap();

    let output = TestRepo::run_at_with_user_data_dir(&linked, ["clear"], repo.user_data_dir());
    assert_eq!(
        output.status.code(),
        Some(0),
        "clear failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("commits: 1"));
    assert!(stdout.contains("semantic vectors: 0"));
    assert!(stdout.contains("cache.sqlite"));
    assert!(stdout.contains("after: 0 bytes"));
    assert!(stdout.contains("cache.lock"));
    assert!(cache_lock.is_file(), "clear removed the coordination lock");
    assert_eq!(repo.head(), head);
    assert_eq!(git_stdout(repo.dir.path(), ["show-ref"]), refs);
    assert_eq!(fs::read(marker).unwrap(), b"must survive clear");

    let mut remaining = fs::read_dir(repo.cache_dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    remaining.sort();
    assert_eq!(remaining, [".gitignore", "cache.lock"]);

    let index = TestRepo::run_at_with_user_data_dir(&linked, ["index"], repo.user_data_dir());
    assert_eq!(
        index.status.code(),
        Some(0),
        "reindex failed: {}",
        String::from_utf8_lossy(&index.stderr)
    );
    assert!(repo.cache_dir().join("cache.sqlite").is_file());
    let query = repo.run(["search", "preservation"]);
    assert_eq!(
        query.status.code(),
        Some(0),
        "query after reindex failed: {}",
        String::from_utf8_lossy(&query.stderr)
    );
    assert!(String::from_utf8_lossy(&query.stdout).contains("Clear reindex preservation marker"));
    drop(worktree_parent);
}

#[test]
fn clear_waits_for_shared_and_exclusive_locks_across_worktrees() {
    let repo = TestRepo::new();
    commit(&repo, "lock marker\n", "Clear lock marker");
    repo.index();
    let (_worktree_parent, linked) = add_linked_worktree(&repo);

    for shared in [true, false] {
        let ready_path = repo.dir.path().join(if shared {
            "shared-ready"
        } else {
            "exclusive-ready"
        });
        let holder = Command::new(env::current_exe().unwrap())
            .args(["--exact", "cache_lock_holder", "--nocapture"])
            .current_dir(repo.dir.path())
            .env("GITSCRY_LOCK_HOLDER", "1")
            .env("GITSCRY_LOCK_SHARED", if shared { "1" } else { "0" })
            .env("GITSCRY_LOCK_PATH", repo.cache_dir().join("cache.lock"))
            .env("GITSCRY_LOCK_READY", &ready_path)
            .env("GITSCRY_LOCK_HOLD_MILLIS", "300")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        for _ in 0..200 {
            if ready_path.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(ready_path.exists(), "lock holder did not start");

        let mut clear = TestRepo::command_at(&linked, repo.user_data_dir())
            .args(["clear", "--dry-run"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        thread::sleep(Duration::from_millis(70));
        assert!(
            clear.try_wait().unwrap().is_none(),
            "clear skipped the lock"
        );
        let output = clear.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("Waiting for another GitScry process")
        );
        assert!(holder.wait_with_output().unwrap().status.success());
    }
}

#[test]
fn cache_lock_holder() {
    let Some(lock_path) = env::var_os("GITSCRY_LOCK_PATH") else {
        return;
    };
    let ready_path = env::var_os("GITSCRY_LOCK_READY").expect("lock ready path");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(lock_path)
        .expect("open cache lock");
    if env::var("GITSCRY_LOCK_SHARED").ok().as_deref() == Some("1") {
        lock.lock_shared().expect("hold shared cache lock");
    } else {
        lock.lock().expect("hold exclusive cache lock");
    }
    fs::write(ready_path, b"ready").expect("signal lock holder");
    let millis = env::var("GITSCRY_LOCK_HOLD_MILLIS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(300);
    thread::sleep(Duration::from_millis(millis));
}
