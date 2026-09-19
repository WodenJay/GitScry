use std::{
    env,
    ffi::OsStr,
    fs::{self, OpenOptions},
    path::Path,
    process::Command,
    thread,
    time::Duration,
};

use rusqlite::Connection;
use tempfile::TempDir;

struct TestRepo {
    dir: TempDir,
}

impl TestRepo {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("create temporary repository");
        git(dir.path(), ["init", "--initial-branch=main"]);
        git(dir.path(), ["config", "user.name", "GitScry Test"]);
        git(
            dir.path(),
            ["config", "user.email", "gitscry@example.invalid"],
        );
        Self { dir }
    }

    fn commit(&self, path: &str, contents: &[u8], message: &str) {
        fs::write(self.dir.path().join(path), contents).expect("write tracked file");
        git(self.dir.path(), ["add", path]);
        git(self.dir.path(), ["commit", "-m", message]);
    }

    fn head(&self) -> String {
        git_stdout(self.dir.path(), ["rev-parse", "HEAD"])
    }

    fn run(&self) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_gitscry"))
            .arg("index")
            .current_dir(self.dir.path())
            .output()
            .expect("run gitscry")
    }
}

fn git<I, S>(cwd: &Path, args: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_stdout<I, S>(cwd: &Path, args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn first_index_publishes_complete_cache() {
    let repo = TestRepo::new();
    repo.commit("hello.txt", b"hello\n", "initial commit");
    fs::create_dir(repo.dir.path().join(".gitscry")).expect("create cache directory");
    fs::write(repo.dir.path().join(".gitscry/.gitignore"), "keep-me\n").expect("seed ignore file");

    let output = repo.run();

    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "Indexed 1 commit.\n"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("Indexing local history"));
    assert_eq!(
        fs::read_to_string(repo.dir.path().join(".gitscry/.gitignore")).unwrap(),
        "keep-me\n*\n"
    );

    let cache = Connection::open(repo.dir.path().join(".gitscry/cache.sqlite")).unwrap();
    assert_eq!(
        cache
            .query_row("SELECT COUNT(*) FROM commits", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        cache
            .query_row(
                "SELECT value FROM metadata WHERE key = 'completed_tip'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        repo.head()
    );
}

#[test]
fn root_merge_and_binary_history_are_persisted() {
    let repo = TestRepo::new();
    repo.commit("root.txt", b"root\n", "root");
    let root = repo.head();
    git(repo.dir.path(), ["checkout", "-b", "side"]);
    repo.commit("side.txt", b"side\n", "side");
    let side = repo.head();
    git(repo.dir.path(), ["checkout", "main"]);
    repo.commit("main.txt", b"main\n", "main");
    let main = repo.head();
    git(repo.dir.path(), ["merge", "--no-ff", "side", "-m", "merge"]);
    let merge = repo.head();
    repo.commit("binary.bin", b"binary\0contents", "binary");
    let binary = repo.head();

    let output = repo.run();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cache = Connection::open(repo.dir.path().join(".gitscry/cache.sqlite")).unwrap();
    assert_eq!(
        cache
            .query_row("SELECT COUNT(*) FROM commits", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        5
    );
    let root_change: (String, Option<Vec<u8>>, Vec<u8>) = cache
        .query_row(
            "SELECT status, old_path, new_path FROM changes WHERE commit_oid = ?1",
            [&root],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(root_change, ("A".to_owned(), None, b"root.txt".to_vec()));

    let parents: Vec<String> = cache
        .prepare("SELECT parent_oid FROM commit_parents WHERE commit_oid = ?1 ORDER BY position")
        .unwrap()
        .query_map([&merge], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(parents, vec![main, side]);
    let merge_paths: Vec<Vec<u8>> = cache
        .prepare("SELECT new_path FROM changes WHERE commit_oid = ?1 ORDER BY ordinal")
        .unwrap()
        .query_map([&merge], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(merge_paths, vec![b"side.txt".to_vec()]);

    assert_eq!(
        cache
            .query_row(
                "SELECT COUNT(*) FROM changes WHERE commit_oid = ?1",
                [&binary],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
    assert_eq!(
        cache
            .query_row(
                "SELECT COUNT(*) FROM hunks WHERE commit_oid = ?1",
                [&binary],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
}

#[test]
fn ambiguous_default_branch_fails_without_publishing() {
    let repo = TestRepo::new();
    repo.commit("file.txt", b"content\n", "initial");
    git(repo.dir.path(), ["branch", "master"]);

    let output = repo.run();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("both main and master exist"));
    assert!(!repo.dir.path().join(".gitscry/cache.sqlite").exists());
}

#[test]
fn origin_head_wins_over_ambiguous_local_defaults() {
    let repo = TestRepo::new();
    repo.commit("file.txt", b"content\n", "initial");
    git(repo.dir.path(), ["branch", "master"]);
    git(
        repo.dir.path(),
        ["update-ref", "refs/remotes/origin/main", "main"],
    );
    git(
        repo.dir.path(),
        [
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );

    let output = repo.run();

    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn current_branch_is_not_used_as_default() {
    let repo = TestRepo::new();
    repo.commit("file.txt", b"content\n", "initial");
    git(repo.dir.path(), ["branch", "-m", "topic"]);

    let output = repo.run();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no default branch"));
    assert!(!repo.dir.path().join(".gitscry/cache.sqlite").exists());
}

#[test]
fn bare_repository_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), ["init", "--bare"]);

    let output = Command::new(env!("CARGO_BIN_EXE_gitscry"))
        .arg("index")
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("bare repositories"));
    assert!(!dir.path().join(".gitscry/cache.sqlite").exists());
}

#[test]
fn preparation_failure_does_not_publish_a_cache() {
    let repo = TestRepo::new();
    repo.commit("file.txt", b"content\n", "initial");
    fs::write(repo.dir.path().join(".gitscry"), "not a directory").unwrap();

    let output = repo.run();

    assert_eq!(output.status.code(), Some(1));
    assert!(!repo.dir.path().join(".gitscry/cache.sqlite").exists());
}

#[test]
fn invalid_cli_input_exits_two() {
    let output = Command::new(env!("CARGO_BIN_EXE_gitscry"))
        .args(["index", "--unexpected"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unexpected"));
}

#[test]
fn help_is_successful_output() {
    let output = Command::new(env!("CARGO_BIN_EXE_gitscry"))
        .arg("--help")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
    assert!(output.stderr.is_empty());
}

#[test]
fn shallow_merge_keeps_parents_and_reindexes_after_deepening() {
    let source = TestRepo::new();
    source.commit("root.txt", b"root\n", "root");
    git(source.dir.path(), ["checkout", "-b", "side"]);
    source.commit("side.txt", b"side\n", "side");
    git(source.dir.path(), ["checkout", "main"]);
    source.commit("main.txt", b"main\n", "main");
    git(
        source.dir.path(),
        ["merge", "--no-ff", "side", "-m", "merge"],
    );
    let merge = source.head();

    let parent = tempfile::tempdir().unwrap();
    let clone = parent.path().join("clone");
    let cloned = Command::new("git")
        .args(["clone", "--depth", "1", "--no-local"])
        .arg(source.dir.path())
        .arg(&clone)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        cloned.status.success(),
        "{}",
        String::from_utf8_lossy(&cloned.stderr)
    );

    let first = Command::new(env!("CARGO_BIN_EXE_gitscry"))
        .arg("index")
        .current_dir(&clone)
        .output()
        .unwrap();
    assert_eq!(
        first.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let cache_path = clone.join(".gitscry/cache.sqlite");
    let cache = Connection::open(&cache_path).unwrap();
    assert_eq!(
        cache
            .query_row(
                "SELECT COUNT(*) FROM commit_parents WHERE commit_oid = ?1",
                [&merge],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        2
    );
    assert_eq!(
        cache
            .query_row(
                "SELECT COUNT(*) FROM changes WHERE commit_oid = ?1",
                [&merge],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
    drop(cache);

    git(&clone, ["fetch", "--unshallow"]);
    let second = Command::new(env!("CARGO_BIN_EXE_gitscry"))
        .arg("index")
        .current_dir(&clone)
        .output()
        .unwrap();
    assert_eq!(
        second.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let cache = Connection::open(cache_path).unwrap();
    assert_eq!(
        cache
            .query_row("SELECT COUNT(*) FROM commits", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        4
    );
    let merge_paths: Vec<Vec<u8>> = cache
        .prepare("SELECT new_path FROM changes WHERE commit_oid = ?1 ORDER BY ordinal")
        .unwrap()
        .query_map([&merge], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(merge_paths, vec![b"side.txt".to_vec()]);
}

#[test]
fn fresh_index_is_quiet_and_reports_completed_count() {
    let repo = TestRepo::new();
    repo.commit("history.txt", b"one\n", "Initial history");

    let first = repo.run();
    assert_eq!(first.status.code(), Some(0));
    let second = repo.run();
    assert_eq!(second.status.code(), Some(0));
    assert_eq!(second.stdout, b"Indexed 1 commit.\n");
    assert!(second.stderr.is_empty());
}

#[test]
fn fast_forward_updates_one_completed_generation() {
    let repo = TestRepo::new();
    repo.commit("history.txt", b"one\n", "Initial history");
    let first = repo.head();
    assert_eq!(repo.run().status.code(), Some(0));

    repo.commit("history.txt", b"two\n", "Second history");
    let second = repo.run();
    assert_eq!(second.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&second.stderr).contains("Indexing local history"));

    let cache = Connection::open(repo.dir.path().join(".gitscry/cache.sqlite")).unwrap();
    assert_eq!(
        cache
            .query_row("SELECT COUNT(*) FROM commits", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        cache
            .query_row(
                "SELECT value FROM metadata WHERE key = 'completed_commit_count'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "2"
    );
    assert_eq!(
        cache
            .query_row(
                "SELECT COUNT(*) FROM changes WHERE commit_oid = ?1",
                [&first],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
}

#[test]
fn non_fast_forward_rebuild_removes_unreachable_commits() {
    let repo = TestRepo::new();
    repo.commit("history.txt", b"one\n", "Initial history");
    let base = repo.head();
    repo.commit("history.txt", b"two\n", "Old second history");
    let old_tip = repo.head();
    assert_eq!(repo.run().status.code(), Some(0));

    git(repo.dir.path(), ["reset", "--hard", &base]);
    repo.commit("history.txt", b"replacement\n", "Replacement history");
    let new_tip = repo.head();
    let output = repo.run();
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Indexing local history"));

    let cache = Connection::open(repo.dir.path().join(".gitscry/cache.sqlite")).unwrap();
    assert_eq!(
        cache
            .query_row(
                "SELECT COUNT(*) FROM commits WHERE oid = ?1",
                [&old_tip],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
    assert_eq!(
        cache
            .query_row(
                "SELECT COUNT(*) FROM commits WHERE oid = ?1",
                [&new_tip],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
}

#[test]
fn default_ref_change_rebuilds_the_pinned_generation() {
    let repo = TestRepo::new();
    repo.commit("history.txt", b"main\n", "Main history");
    assert_eq!(repo.run().status.code(), Some(0));

    git(repo.dir.path(), ["checkout", "-b", "feature"]);
    repo.commit("feature.txt", b"feature\n", "Feature history");
    let feature_tip = repo.head();
    git(repo.dir.path(), ["checkout", "main"]);
    git(
        repo.dir.path(),
        ["update-ref", "refs/remotes/origin/feature", &feature_tip],
    );
    git(
        repo.dir.path(),
        [
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/feature",
        ],
    );

    let output = repo.run();
    assert_eq!(output.status.code(), Some(0));
    let cache = Connection::open(repo.dir.path().join(".gitscry/cache.sqlite")).unwrap();
    assert_eq!(
        cache
            .query_row(
                "SELECT value FROM metadata WHERE key = 'default_ref'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "refs/remotes/origin/feature"
    );
    assert_eq!(
        cache
            .query_row(
                "SELECT value FROM metadata WHERE key = 'completed_tip'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        feature_tip
    );
}

#[test]
fn restores_hunks_after_a_missing_blob_returns() {
    let repo = TestRepo::new();
    repo.commit("history.txt", b"one\n", "First history");
    let first = repo.head();
    repo.commit("history.txt", b"two\n", "Second history");
    repo.commit("history.txt", b"three\n", "Third history");

    let blob = git_stdout(repo.dir.path(), ["ls-tree", &first, "history.txt"]);
    let blob = blob.split_whitespace().nth(2).unwrap().to_owned();
    let object_path = repo
        .dir
        .path()
        .join(".git/objects")
        .join(&blob[..2])
        .join(&blob[2..]);
    fs::remove_file(object_path).expect("remove loose blob");

    let first_index = repo.run();
    assert_eq!(first_index.status.code(), Some(0));
    let cache_path = repo.dir.path().join(".gitscry/cache.sqlite");
    let cache = Connection::open(&cache_path).unwrap();
    let incomplete_hunks: i64 = cache
        .query_row("SELECT COUNT(*) FROM hunks", [], |row| row.get(0))
        .unwrap();
    assert!(incomplete_hunks < 3);
    drop(cache);

    fs::write(repo.dir.path().join("restore.txt"), b"one\n").unwrap();
    git(repo.dir.path(), ["hash-object", "-w", "restore.txt"]);
    fs::remove_file(repo.dir.path().join("restore.txt")).unwrap();

    let restored = repo.run();
    assert_eq!(restored.status.code(), Some(0));
    let cache = Connection::open(cache_path).unwrap();
    assert_eq!(
        cache
            .query_row("SELECT COUNT(*) FROM hunks", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        3
    );
}

#[test]
fn missing_cached_commit_fails_the_next_preparation() {
    let repo = TestRepo::new();
    repo.commit("history.txt", b"one\n", "First history");
    let missing_commit = repo.head();
    repo.commit("history.txt", b"two\n", "Second history");
    assert_eq!(repo.run().status.code(), Some(0));
    let cache_path = repo.dir.path().join(".gitscry/cache.sqlite");
    let completed_tip = repo.head();

    let object_path = repo
        .dir
        .path()
        .join(".git/objects")
        .join(&missing_commit[..2])
        .join(&missing_commit[2..]);
    fs::remove_file(object_path).expect("remove loose commit");

    let output = repo.run();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Git command failed"));
    let cache = Connection::open(cache_path).unwrap();
    assert_eq!(
        cache
            .query_row("SELECT COUNT(*) FROM commits", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        cache
            .query_row(
                "SELECT value FROM metadata WHERE key = 'completed_tip'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        completed_tip
    );
}

#[test]
fn cache_lock_holder() {
    if env::var_os("GITSCRY_LOCK_HOLDER").is_none() {
        return;
    }
    let lock_path = env::var_os("GITSCRY_LOCK_PATH").expect("lock path");
    let ready_path = env::var_os("GITSCRY_LOCK_READY").expect("ready path");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(lock_path)
        .expect("open cache lock");
    lock.lock().expect("lock cache");
    fs::write(ready_path, b"ready").expect("signal lock holder");
    thread::sleep(Duration::from_secs(1));
}

#[test]
fn stale_observer_waits_once_for_an_existing_writer() {
    let repo = TestRepo::new();
    repo.commit("history.txt", b"one\n", "Initial history");
    assert_eq!(repo.run().status.code(), Some(0));

    let ready_path = repo.dir.path().join("lock-ready");
    let holder = Command::new(env::current_exe().unwrap())
        .args(["--exact", "cache_lock_holder", "--nocapture"])
        .current_dir(repo.dir.path())
        .env("GITSCRY_LOCK_HOLDER", "1")
        .env(
            "GITSCRY_LOCK_PATH",
            repo.dir.path().join(".gitscry/cache.lock"),
        )
        .env("GITSCRY_LOCK_READY", &ready_path)
        .spawn()
        .expect("spawn lock holder");
    for _ in 0..200 {
        if ready_path.exists() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(ready_path.exists(), "lock holder did not start");

    let output = repo.run();
    let _ = holder.wait_with_output().expect("wait for lock holder");
    assert_eq!(output.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr
            .matches("Waiting for another GitScry process...")
            .count(),
        1
    );
}

#[test]
fn recovers_a_previous_generation_after_interrupted_replace() {
    let repo = TestRepo::new();
    repo.commit("history.txt", b"one\n", "Initial history");
    assert_eq!(repo.run().status.code(), Some(0));

    let cache = repo.dir.path().join(".gitscry/cache.sqlite");
    let previous = repo.dir.path().join(".gitscry/cache.sqlite.previous");
    fs::rename(&cache, &previous).expect("simulate interrupted replacement");

    let output = repo.run();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"Indexed 1 commit.\n");
    assert!(output.stderr.is_empty());
    assert!(cache.is_file());
    assert!(!previous.exists());
}

#[test]
fn concurrent_indexers_leave_one_complete_generation() {
    let repo = TestRepo::new();
    repo.commit("history.txt", b"one\n", "Initial history");

    let first = Command::new(env!("CARGO_BIN_EXE_gitscry"))
        .arg("index")
        .current_dir(repo.dir.path())
        .spawn()
        .unwrap();
    let second = Command::new(env!("CARGO_BIN_EXE_gitscry"))
        .arg("index")
        .current_dir(repo.dir.path())
        .spawn()
        .unwrap();
    let first = first.wait_with_output().unwrap();
    let second = second.wait_with_output().unwrap();

    assert_eq!(first.status.code(), Some(0));
    assert_eq!(second.status.code(), Some(0));
    let cache = Connection::open(repo.dir.path().join(".gitscry/cache.sqlite")).unwrap();
    assert_eq!(
        cache
            .query_row("SELECT COUNT(*) FROM commits", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn concurrent_readers_return_identical_material() {
    let repo = TestRepo::new();
    repo.commit("history.txt", b"one\n", "Initial history");
    assert_eq!(repo.run().status.code(), Some(0));

    let first = Command::new(env!("CARGO_BIN_EXE_gitscry"))
        .args(["search", "history"])
        .current_dir(repo.dir.path())
        .spawn()
        .unwrap();
    let second = Command::new(env!("CARGO_BIN_EXE_gitscry"))
        .args(["search", "history"])
        .current_dir(repo.dir.path())
        .spawn()
        .unwrap();
    let first = first.wait_with_output().unwrap();
    let second = second.wait_with_output().unwrap();

    assert_eq!(first.status.code(), Some(0));
    assert_eq!(second.status.code(), Some(0));
    assert_eq!(first.stdout, second.stdout);
}

#[test]
fn reduced_shallow_history_rebuilds_without_reusing_rows() {
    let source = TestRepo::new();
    for (index, contents) in [
        (1, &b"one\n"[..]),
        (2, &b"two\n"[..]),
        (3, &b"three\n"[..]),
        (4, &b"four\n"[..]),
    ] {
        source.commit("history.txt", contents, &format!("History {index}"));
    }

    let parent = tempfile::tempdir().unwrap();
    let clone = parent.path().join("clone");
    let cloned = Command::new("git")
        .args(["clone", "--depth", "2", "--no-local"])
        .arg(source.dir.path())
        .arg(&clone)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(cloned.status.success());

    let first = Command::new(env!("CARGO_BIN_EXE_gitscry"))
        .arg("index")
        .current_dir(&clone)
        .output()
        .unwrap();
    assert_eq!(first.status.code(), Some(0));
    let cache_path = clone.join(".gitscry/cache.sqlite");
    let cache = Connection::open(&cache_path).unwrap();
    assert_eq!(
        cache
            .query_row("SELECT COUNT(*) FROM commits", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    drop(cache);

    git(&clone, ["fetch", "--depth=1"]);
    let second = Command::new(env!("CARGO_BIN_EXE_gitscry"))
        .arg("index")
        .current_dir(&clone)
        .output()
        .unwrap();
    assert_eq!(second.status.code(), Some(0));
    let cache = Connection::open(cache_path).unwrap();
    assert_eq!(
        cache
            .query_row("SELECT COUNT(*) FROM commits", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}
