use std::{ffi::OsStr, fs, path::Path, process::Command};

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
