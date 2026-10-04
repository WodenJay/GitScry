use std::collections::HashMap;
mod support;

use std::{
    env,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};
use tempfile::TempDir;

use support::{TestRepo, git, git_command, git_common_dir, git_stdout};

use rusqlite::Connection;
use sha2::Digest;

impl TestRepo {
    fn commit(&self, path: &str, contents: &[u8], message: &str) {
        fs::write(self.dir.path().join(path), contents).expect("write tracked file");
        git(self.dir.path(), ["add", path]);
        git(self.dir.path(), ["commit", "-m", message]);
    }
}

struct IsolatedExecutable {
    _install: TempDir,
    user_data: TempDir,
    path: PathBuf,
}

impl IsolatedExecutable {
    fn new() -> Self {
        let source = PathBuf::from(env!("CARGO_BIN_EXE_gitscry"));
        let install = tempfile::tempdir_in(source.parent().expect("binary directory"))
            .expect("create isolated installation");
        let path = install
            .path()
            .join(source.file_name().expect("binary filename"));
        // Hard-link the immutable binary: no writable descriptor can race a parallel exec.
        fs::hard_link(source, &path).expect("link executable without runtime");
        Self {
            _install: install,
            path,
            user_data: tempfile::tempdir().expect("create isolated user data directory"),
        }
    }

    fn run(&self, cwd: &Path, args: &[&str]) -> std::process::Output {
        Command::new(&self.path)
            .args(args)
            .current_dir(cwd)
            .env("LOCALAPPDATA", self.user_data.path())
            .env("XDG_DATA_HOME", self.user_data.path().join("xdg"))
            .env("HOME", self.user_data.path().join("home"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", cwd.join("global-config"))
            .output()
            .expect("run isolated gitscry")
    }
}

#[test]
fn first_index_publishes_complete_cache() {
    let repo = TestRepo::new();
    repo.commit("hello.txt", b"hello\n", "initial commit");
    fs::create_dir(repo.cache_dir()).expect("create cache directory");
    fs::write(repo.cache_dir().join(".gitignore"), "keep-me\n").expect("seed ignore file");

    let output = repo.run(["index"]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "Indexed 1 commit reachable from current HEAD.\n"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Indexing history reachable from current HEAD")
    );
    assert_eq!(
        fs::read_to_string(repo.cache_dir().join(".gitignore")).unwrap(),
        "keep-me\n*\n"
    );

    let cache = Connection::open(repo.cache_dir().join("cache.sqlite")).unwrap();
    assert_eq!(
        cache
            .query_row("SELECT COUNT(*) FROM commits", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let semantic_state: (String, String, i64) = cache
        .query_row(
            "SELECT
                (SELECT value FROM metadata WHERE key = 'semantic_enabled'),
                (SELECT value FROM metadata WHERE key = 'semantic_ready'),
                (SELECT COUNT(*) FROM semantic_vectors)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(semantic_state, ("0".to_owned(), "0".to_owned(), 0));
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
fn failed_incremental_write_keeps_the_published_generation() {
    let repo = TestRepo::new();
    repo.commit("hello.txt", b"hello\n", "initial commit");
    assert!(repo.run(["index"]).status.success());
    let published_tip = repo.head();
    repo.commit("hello.txt", b"hello again\n", "extend greeting");

    let cache_path = repo.cache_dir().join("cache.sqlite");
    let cache = Connection::open(&cache_path).unwrap();
    cache
        .execute_batch(
            "CREATE TRIGGER reject_hunk BEFORE INSERT ON hunks
         BEGIN SELECT RAISE(ABORT, 'injected hunk write failure'); END;",
        )
        .unwrap();
    drop(cache);

    let failed = repo.run(["index"]);
    assert_eq!(failed.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&failed.stderr).contains("injected hunk write failure"));
    let cache = Connection::open(&cache_path).unwrap();
    let (tip, count): (String, i64) = cache
        .query_row(
            "SELECT (SELECT value FROM metadata WHERE key = 'completed_tip'),
                (SELECT COUNT(*) FROM commits)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(tip, published_tip);
    assert_eq!(count, 1);
    let completed_count: String = cache
        .query_row(
            "SELECT value FROM metadata WHERE key = 'completed_commit_count'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(completed_count, "1");
    let orphan_documents: i64 = cache
        .query_row(
            "SELECT COUNT(*) FROM search_fts WHERE rowid NOT IN (SELECT commit_id FROM commits)",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(orphan_documents, 0);
    cache.execute_batch("DROP TRIGGER reject_hunk").unwrap();
    drop(cache);

    let retry = repo.run(["index"]);
    assert!(
        retry.status.success(),
        "{}",
        String::from_utf8_lossy(&retry.stderr)
    );
    let cache = Connection::open(&cache_path).unwrap();
    let tip: String = cache
        .query_row(
            "SELECT value FROM metadata WHERE key = 'completed_tip'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tip, repo.head());
    let count: i64 = cache
        .query_row("SELECT COUNT(*) FROM commits", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn gitlink_entries_are_not_reported_as_missing_objects() {
    let repo = TestRepo::new();
    repo.commit("README.md", b"root\n", "root");
    let gitlink_oid = "1111111111111111111111111111111111111111";
    let gitlink = format!("160000,{gitlink_oid},vendor/submodule");
    git(
        repo.dir.path(),
        ["update-index", "--add", "--cacheinfo", gitlink.as_str()],
    );
    git(repo.dir.path(), ["commit", "-m", "add submodule link"]);

    let output = repo.run(["index"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let all_output = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !all_output.contains("some local objects are missing"),
        "{all_output}"
    );

    let cache = Connection::open(repo.cache_dir().join("cache.sqlite")).unwrap();
    assert_eq!(
        cache
            .query_row("SELECT COUNT(*) FROM missing_objects", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
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

    let output = repo.run(["index"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cache = Connection::open(repo.cache_dir().join("cache.sqlite")).unwrap();
    assert_eq!(
        cache
            .query_row("SELECT COUNT(*) FROM commits", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        5
    );
    let root_change: (String, Option<Vec<u8>>, Vec<u8>) = cache
        .query_row(
            "SELECT ch.status, ch.old_path, ch.new_path
             FROM changes AS ch
             JOIN commits AS c ON c.commit_id = ch.commit_id
             WHERE c.oid = ?1",
            [&root],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(root_change, ("A".to_owned(), None, b"root.txt".to_vec()));

    let parents: Vec<String> = cache
        .prepare(
            "SELECT COALESCE(parent.oid, p.external_oid)
             FROM commit_parents AS p
             JOIN commits AS c ON c.commit_id = p.commit_id
             LEFT JOIN commits AS parent ON parent.commit_id = p.parent_id
             WHERE c.oid = ?1
             ORDER BY p.position",
        )
        .unwrap()
        .query_map([&merge], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(parents, vec![main, side]);
    let merge_paths: Vec<Vec<u8>> = cache
        .prepare(
            "SELECT ch.new_path
             FROM changes AS ch
             JOIN commits AS c ON c.commit_id = ch.commit_id
             WHERE c.oid = ?1
             ORDER BY ch.ordinal",
        )
        .unwrap()
        .query_map([&merge], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(merge_paths, vec![b"side.txt".to_vec()]);

    assert_eq!(
        cache
            .query_row(
                "SELECT COUNT(*)
                 FROM changes AS ch
                 JOIN commits AS c ON c.commit_id = ch.commit_id
                 WHERE c.oid = ?1",
                [&binary],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
    assert_eq!(
        cache
            .query_row(
                "SELECT COUNT(*)
                 FROM hunks AS h
                 JOIN changes AS ch ON ch.change_id = h.change_id
                 JOIN commits AS c ON c.commit_id = ch.commit_id
                 WHERE c.oid = ?1",
                [&binary],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
}

#[test]
fn index_uses_current_head_when_default_branch_is_ambiguous() {
    let repo = TestRepo::new();
    repo.commit("file.txt", b"content\n", "initial");
    let head = repo.head();
    git(repo.dir.path(), ["branch", "master"]);

    let output = repo.run(["index"]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cache = Connection::open(repo.cache_dir().join("cache.sqlite")).unwrap();
    let cached_head: String = cache
        .query_row("SELECT oid FROM commits", [], |row| row.get(0))
        .unwrap();
    assert_eq!(cached_head, head);
}

#[test]
fn indexing_uses_current_head_instead_of_remote_default() {
    let repo = TestRepo::new();
    repo.commit("head.txt", b"head\n", "current HEAD");
    let head_tip = repo.head();
    git(repo.dir.path(), ["branch", "master"]);

    git(repo.dir.path(), ["checkout", "--orphan", "remote-default"]);
    git(repo.dir.path(), ["rm", "-rf", "."]);
    repo.commit("default.txt", b"default\n", "remote default");
    let default_tip = repo.head();
    git(
        repo.dir.path(),
        ["update-ref", "refs/remotes/origin/default", &default_tip],
    );
    git(
        repo.dir.path(),
        [
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/default",
        ],
    );
    git(repo.dir.path(), ["checkout", "main"]);

    let output = repo.run(["index"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cache = Connection::open(repo.cache_dir().join("cache.sqlite")).unwrap();
    assert_eq!(
        cache
            .query_row(
                "SELECT COUNT(*) FROM commits WHERE oid = ?1",
                [&head_tip],
                |row| { row.get::<_, i64>(0) }
            )
            .unwrap(),
        1
    );
    assert_eq!(
        cache
            .query_row(
                "SELECT COUNT(*) FROM commits WHERE oid = ?1",
                [&default_tip],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0,
        "index must use current HEAD rather than remote/HEAD"
    );
}

#[test]
fn sole_local_branch_is_used_as_default() {
    let repo = TestRepo::new();
    repo.commit("file.txt", b"content\n", "initial");
    git(repo.dir.path(), ["branch", "-m", "topic"]);

    let output = repo.run(["index"]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "Indexed 1 commit reachable from current HEAD.\n"
    );
    assert!(repo.cache_dir().join("cache.sqlite").exists());
}

#[test]
fn bare_repository_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), ["init", "--bare"]);
    let user_data = tempfile::tempdir().expect("create isolated user data");

    let output = support::isolated_gitscry_command(user_data.path())
        .arg("index")
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("bare repositories"));
    assert!(!dir.path().join("gitscry/cache.sqlite").exists());
}

#[test]
fn preparation_failure_does_not_publish_a_cache() {
    let repo = TestRepo::new();
    repo.commit("file.txt", b"content\n", "initial");
    fs::write(repo.cache_dir(), "not a directory").unwrap();

    let output = repo.run(["index"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(!repo.cache_dir().join("cache.sqlite").exists());
}

#[test]
fn semantic_index_options_are_exposed_and_mutually_exclusive() {
    let user_data = tempfile::tempdir().expect("create isolated user data");
    let help = support::isolated_gitscry_command(user_data.path())
        .args(["index", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(help.contains("--semantic"), "{help}");
    assert!(help.contains("--no-semantic"), "{help}");

    let conflict = support::isolated_gitscry_command(user_data.path())
        .args(["index", "--semantic", "--no-semantic"])
        .output()
        .unwrap();
    assert_eq!(conflict.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("cannot be used with"));
}

#[test]
fn ordinary_query_refreshes_enabled_semantics_best_effort_across_branches() {
    let repo = TestRepo::new();
    repo.commit("first.txt", b"first\n", "initial commit");
    repo.index();

    let isolated = IsolatedExecutable::new();
    let disabled = isolated.run(repo.dir.path(), &["search", "first"]);
    assert_eq!(disabled.status.code(), Some(0));
    assert!(!String::from_utf8_lossy(&disabled.stderr).contains("automatic semantic refresh"));
    assert!(!isolated.user_data.path().join("GitScry/semantic").exists());

    let cache_path = repo.cache_dir().join("cache.sqlite");
    let cache = Connection::open(&cache_path).unwrap();
    cache
        .execute(
            "UPDATE metadata SET value = '1' WHERE key = 'semantic_enabled'",
            [],
        )
        .unwrap();
    drop(cache);
    git(repo.dir.path(), ["switch", "-c", "side"]);
    repo.commit("second.txt", b"second\n", "second commit");
    let side = repo.head();

    let output = isolated.run(repo.dir.path(), &["search", "first"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let warning = String::from_utf8_lossy(&output.stderr);
    assert!(
        warning.contains("automatic semantic refresh failed"),
        "{warning}"
    );
    assert!(
        warning.contains("pinned semantic model resource"),
        "{warning}"
    );

    git(repo.dir.path(), ["switch", "main"]);
    let fresh = isolated.run(repo.dir.path(), &["search", "first"]);
    assert_eq!(
        fresh.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&fresh.stderr)
    );
    let warning = String::from_utf8_lossy(&fresh.stderr);
    assert!(
        warning.contains("automatic semantic refresh failed"),
        "{warning}"
    );
    assert!(
        warning.contains("pinned semantic model resource"),
        "{warning}"
    );
    assert!(
        !warning.contains("published cache changed after HEAD was pinned"),
        "{warning}"
    );

    let cache = Connection::open(&cache_path).unwrap();
    let metadata = |key: &str| {
        cache
            .query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| {
                row.get::<_, String>(0)
            })
            .unwrap()
    };
    assert_eq!(metadata("completed_tip"), side);
    assert_ne!(metadata("completed_tip"), repo.head());
    assert_eq!(metadata("completed_commit_count"), "2");
    assert_eq!(metadata("semantic_enabled"), "1");
    assert_eq!(metadata("semantic_ready"), "0");
    assert!(!isolated.user_data.path().join("GitScry/semantic").exists());
}

#[test]
fn semantic_index_failure_preserves_history_until_explicitly_disabled() {
    let repo = TestRepo::new();
    repo.commit("hello.txt", b"hello\n", "initial commit");
    let missing_runtime_executable = IsolatedExecutable::new();
    let run_with_missing_runtime =
        |args: &[&str]| missing_runtime_executable.run(repo.dir.path(), args);

    let initial = repo.run(["index"]);
    assert!(
        initial.status.success(),
        "{}",
        String::from_utf8_lossy(&initial.stderr)
    );
    let failed = run_with_missing_runtime(&["index", "--semantic"]);
    assert_eq!(failed.status.code(), Some(1));
    let error = String::from_utf8_lossy(&failed.stderr);
    assert!(
        error.contains("semantic runtime package is unavailable"),
        "{error}"
    );
    assert!(
        error.contains("ordinary history cache is usable"),
        "{error}"
    );

    let cache_path = repo.cache_dir().join("cache.sqlite");
    let cache = Connection::open(&cache_path).unwrap();
    let metadata = |key: &str| {
        cache
            .query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| {
                row.get::<_, String>(0)
            })
            .unwrap()
    };
    assert_eq!(metadata("semantic_enabled"), "1");
    assert_eq!(metadata("semantic_ready"), "0");
    assert_eq!(
        cache
            .query_row("SELECT COUNT(*) FROM semantic_vectors", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(repo.run(["search", "initial"]).status.success());

    repo.commit("next.txt", b"next\n", "forward commit");
    let retried = run_with_missing_runtime(&["index"]);
    assert_eq!(retried.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&retried.stderr).contains("semantic indexing remains enabled"));
    assert_eq!(metadata("completed_commit_count"), "2");

    assert_eq!(
        cache
            .execute(
                "INSERT INTO semantic_vectors(
                    commit_id, commit_oid, source_fingerprint, embedding, input_fingerprint,
                    encoder_fingerprint, runtime_provenance
                ) SELECT commit_id, oid, printf('%064d', 0), zeroblob(1536),
                         printf('%064d', 0), printf('%064d', 0), 'test'
                  FROM commits LIMIT 1",
                [],
            )
            .unwrap(),
        1
    );
    let disabled = repo.run(["index", "--no-semantic"]);
    assert!(
        disabled.status.success(),
        "{}",
        String::from_utf8_lossy(&disabled.stderr)
    );
    let output = String::from_utf8_lossy(&disabled.stdout);
    assert!(output.contains("repository-wide"), "{output}");
    assert!(
        output.contains("all cached semantic vectors were removed"),
        "{output}"
    );
    assert_eq!(metadata("semantic_enabled"), "0");
    assert_eq!(metadata("semantic_ready"), "0");
    assert_eq!(
        cache
            .query_row("SELECT COUNT(*) FROM semantic_vectors", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );

    let ordinary = run_with_missing_runtime(&["index"]);
    assert!(
        ordinary.status.success(),
        "{}",
        String::from_utf8_lossy(&ordinary.stderr)
    );
    let reenabled = run_with_missing_runtime(&["index", "--semantic"]);
    assert_eq!(reenabled.status.code(), Some(1));
    let error = String::from_utf8_lossy(&reenabled.stderr);
    assert!(
        error.contains("semantic indexing remains enabled"),
        "{error}"
    );
    assert_eq!(metadata("semantic_enabled"), "1");
    assert_eq!(metadata("semantic_ready"), "0");
}

#[test]
fn hybrid_readiness_uses_query_scope_instead_of_repository_wide_coverage() {
    let repo = TestRepo::new();
    repo.commit("base.txt", b"base\n", "Base commit");
    let base = repo.head();
    repo.index();

    git(repo.dir.path(), ["switch", "-c", "side"]);
    repo.commit("side.txt", b"side\n", "Side-only commit");
    let side = repo.head();
    repo.index();

    git(repo.dir.path(), ["switch", "main"]);
    repo.commit("main.txt", b"main\n", "Main-only commit");
    let head = repo.head();
    repo.index();

    let cache = Connection::open(repo.cache_dir().join("cache.sqlite")).unwrap();
    for (oid, commit_time) in [(&base, 0_i64), (&side, 0), (&head, 86_400)] {
        cache
            .execute(
                "UPDATE commits SET commit_time = ?1 WHERE oid = ?2",
                rusqlite::params![commit_time, oid],
            )
            .unwrap();
    }
    cache
        .execute(
            "UPDATE metadata SET value = '1' WHERE key = 'semantic_enabled'",
            [],
        )
        .unwrap();
    cache
        .execute(
            "UPDATE metadata SET value = '0' WHERE key = 'semantic_ready'",
            [],
        )
        .unwrap();
    let encoder_fingerprint = expected_encoder_fingerprint();
    for (axis, oid) in [(0, &base), (1, &head)] {
        let commit_id = cache
            .query_row(
                "SELECT commit_id FROM commits WHERE oid = ?1",
                [oid],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        let source_fingerprint = expected_source_fingerprint(&cache, commit_id);
        cache
            .execute(
                "INSERT INTO semantic_vectors(
                    commit_id, commit_oid, source_fingerprint, embedding, input_fingerprint,
                    encoder_fingerprint, runtime_provenance
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'test')",
                rusqlite::params![
                    commit_id,
                    oid,
                    source_fingerprint,
                    normalized_vector(axis),
                    "c".repeat(64),
                    encoder_fingerprint,
                ],
            )
            .unwrap();
    }
    drop(cache);

    let isolated = IsolatedExecutable::new();
    let assert_resources_missing = |args: &[&str]| {
        let output = isolated.run(repo.dir.path(), args);
        let error = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{error}");
        assert!(
            error.contains("pinned semantic model resource"),
            "{args:?}: {error}"
        );
    };
    let assert_scope_incomplete = |args: &[&str]| {
        let output = isolated.run(repo.dir.path(), args);
        let error = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{error}");
        assert!(
            error.contains("semantic index is missing, stale, or incomplete"),
            "{error}"
        );
    };

    // The side branch has no vectors, but all current-HEAD history does.
    assert_resources_missing(&["search", "marker", "--hybrid"]);
    assert_scope_incomplete(&["search", "marker", "--hybrid", "--to-rev", &side]);
    assert_resources_missing(&["search", "marker", "--hybrid", "--from-rev", &base]);
    assert_resources_missing(&["search", "marker", "--hybrid", "--since", "1970-01-02"]);
}
#[test]
fn semantic_vectors_survive_rebuild_by_commit_identity() {
    let repo = TestRepo::new();
    repo.commit("one.txt", b"one\n", "first commit");
    let first = repo.head();
    repo.commit("two.txt", b"two\n", "second commit");
    let second = repo.head();
    assert!(repo.run(["index"]).status.success());

    let cache_path = repo.cache_dir().join("cache.sqlite");
    let cache = Connection::open(&cache_path).unwrap();
    let encoder_fingerprint = expected_encoder_fingerprint();
    cache
        .execute(
            "UPDATE metadata SET value = '1' WHERE key = 'semantic_enabled'",
            [],
        )
        .unwrap();
    cache
        .execute(
            "UPDATE metadata SET value = ?1 WHERE key = 'semantic_encoder_fingerprint'",
            [&encoder_fingerprint],
        )
        .unwrap();
    cache
        .execute(
            "UPDATE metadata SET value = 'previous-runtime' WHERE key = 'semantic_runtime_provenance'",
            [],
        )
        .unwrap();

    let commits = cache
        .prepare("SELECT commit_id, oid FROM commits ORDER BY position")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let vectors_by_oid = HashMap::from([
        (first.clone(), normalized_vector(0)),
        (second.clone(), normalized_vector(1)),
    ]);
    for (commit_id, oid) in commits {
        let embedding = vectors_by_oid.get(&oid).unwrap();
        let source_fingerprint = expected_source_fingerprint(&cache, commit_id);
        let vector_oid = if oid == second {
            first.as_str()
        } else {
            oid.as_str()
        };
        cache
            .execute(
                "INSERT INTO semantic_vectors(
                    commit_id, commit_oid, source_fingerprint, embedding, input_fingerprint,
                    encoder_fingerprint, runtime_provenance
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    commit_id,
                    vector_oid,
                    source_fingerprint,
                    embedding,
                    "c".repeat(64),
                    encoder_fingerprint,
                    "previous-runtime"
                ],
            )
            .unwrap();
    }
    // Make position order disagree with SHA identity so a rebuild must remap by OID.
    cache
        .execute_batch(
            "UPDATE commits SET position = -1 WHERE position = 0;
             UPDATE commits SET position = 0 WHERE position = 1;
             UPDATE commits SET position = 1 WHERE position = -1",
        )
        .unwrap();
    cache
        .execute(
            "UPDATE metadata SET value = '7' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
    drop(cache);

    let rebuilt = support::isolated_gitscry_command(repo.user_data_dir())
        .arg("index")
        .current_dir(repo.dir.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", repo.dir.path().join("global-config"))
        .output()
        .unwrap();
    assert_eq!(
        rebuilt.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&rebuilt.stderr)
    );

    let cache = Connection::open(&cache_path).unwrap();
    let (ready, count): (String, i64) = cache
        .query_row(
            "SELECT
                (SELECT value FROM metadata WHERE key = 'semantic_ready'),
                (SELECT COUNT(*) FROM semantic_vectors)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((ready.as_str(), count), ("0", 1));
    let preserved = cache
        .prepare(
            "SELECT c.oid, v.embedding FROM commits AS c
             JOIN semantic_vectors AS v USING (commit_id)",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .unwrap()
        .collect::<Result<HashMap<_, _>, _>>()
        .unwrap();
    assert_eq!(
        preserved,
        HashMap::from([(first.clone(), normalized_vector(0))])
    );
    assert!(repo.run(["search", "first"]).status.success());

    let (commit_id, oid): (i64, String) = cache
        .query_row(
            "SELECT commit_id, oid FROM commits WHERE oid = ?1",
            [&second],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    cache
        .execute(
            "INSERT INTO semantic_vectors(
                commit_id, commit_oid, source_fingerprint, embedding, input_fingerprint,
                encoder_fingerprint, runtime_provenance
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                commit_id,
                oid,
                expected_source_fingerprint(&cache, commit_id),
                vectors_by_oid.get(&second).unwrap(),
                "c".repeat(64),
                encoder_fingerprint,
                "previous-runtime"
            ],
        )
        .unwrap();
    drop(cache);

    let resumed = support::isolated_gitscry_command(repo.user_data_dir())
        .arg("index")
        .current_dir(repo.dir.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", repo.dir.path().join("global-config"))
        .output()
        .unwrap();
    assert_eq!(
        resumed.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );

    let cache = Connection::open(&cache_path).unwrap();
    let (ready, count): (String, i64) = cache
        .query_row(
            "SELECT
                (SELECT value FROM metadata WHERE key = 'semantic_ready'),
                (SELECT COUNT(*) FROM semantic_vectors)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((ready.as_str(), count), ("0", 2));
    let published = cache
        .prepare(
            "SELECT c.oid, v.embedding FROM commits AS c
             JOIN semantic_vectors AS v USING (commit_id)",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .unwrap()
        .collect::<Result<HashMap<_, _>, _>>()
        .unwrap();
    assert_eq!(published, vectors_by_oid);
    cache
        .execute_batch(
            "CREATE TABLE semantic_vectors_v7 (
                commit_id INTEGER PRIMARY KEY REFERENCES commits(commit_id) ON DELETE CASCADE,
                embedding BLOB NOT NULL CHECK (length(embedding) = 1536),
                source_fingerprint TEXT NOT NULL CHECK (length(source_fingerprint) = 64),
                input_fingerprint TEXT NOT NULL CHECK (length(input_fingerprint) = 64),
                encoder_fingerprint TEXT NOT NULL CHECK (length(encoder_fingerprint) = 64),
                runtime_provenance TEXT NOT NULL
             ) STRICT;
             INSERT INTO semantic_vectors_v7(
                commit_id, embedding, source_fingerprint, input_fingerprint,
                encoder_fingerprint, runtime_provenance
             )
             SELECT commit_id, embedding, source_fingerprint, input_fingerprint,
                    encoder_fingerprint, runtime_provenance FROM semantic_vectors;
             DROP TABLE semantic_vectors;
             ALTER TABLE semantic_vectors_v7 RENAME TO semantic_vectors;
             UPDATE metadata SET value = '7' WHERE key = 'schema_version';",
        )
        .unwrap();
    drop(cache);
    let legacy_rebuilt = support::isolated_gitscry_command(repo.user_data_dir())
        .arg("index")
        .current_dir(repo.dir.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", repo.dir.path().join("global-config"))
        .output()
        .unwrap();
    assert_eq!(
        legacy_rebuilt.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&legacy_rebuilt.stderr)
    );
    let cache = Connection::open(&cache_path).unwrap();
    let (ready, count): (String, i64) = cache
        .query_row(
            "SELECT
                (SELECT value FROM metadata WHERE key = 'semantic_ready'),
                (SELECT COUNT(*) FROM semantic_vectors)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((ready.as_str(), count), ("0", 2));
    let legacy_vectors = cache
        .prepare(
            "SELECT c.oid, v.embedding FROM commits AS c
             JOIN semantic_vectors AS v USING (commit_id)",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .unwrap()
        .collect::<Result<HashMap<_, _>, _>>()
        .unwrap();
    assert_eq!(legacy_vectors, vectors_by_oid);
    cache
        .execute_batch(
            "PRAGMA ignore_check_constraints = ON;
             UPDATE semantic_vectors SET embedding = zeroblob(4)
             WHERE commit_id = (SELECT MIN(commit_id) FROM semantic_vectors)",
        )
        .unwrap();
    drop(cache);
    let damaged = support::isolated_gitscry_command(repo.user_data_dir())
        .arg("index")
        .current_dir(repo.dir.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", repo.dir.path().join("global-config"))
        .output()
        .unwrap();
    assert_eq!(damaged.status.code(), Some(1));
    let cache = Connection::open(cache_path).unwrap();
    let (ready, count): (String, i64) = cache
        .query_row(
            "SELECT
                (SELECT value FROM metadata WHERE key = 'semantic_ready'),
                (SELECT COUNT(*) FROM semantic_vectors)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((ready.as_str(), count), ("0", 2));
    assert!(repo.run(["search", "first"]).status.success());
}

fn normalized_vector(axis: usize) -> Vec<u8> {
    let mut vector = vec![0.0_f32; 384];
    vector[axis] = 1.0;
    vector.into_iter().flat_map(f32::to_le_bytes).collect()
}

fn expected_encoder_fingerprint() -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"gitscry-semantic-encoder-v2;title=64;paths=32;total=256;field-prefix-chars=4096;path-separator=newline;right-pad;token-types=zero;mask-mean-f32-l2;dimension=384");
    hasher.update(b"8f518e882455312b086101e60691f5e6e2f05c3c");
    for (path, bytes, hash) in [
        (
            "model.onnx",
            90_387_630_u64,
            "bbd7b466f6d58e646fdc2bd5fd67b2f5e93c0b687011bd4548c420f7bd46f0c5",
        ),
        (
            "tokenizer.json",
            711_661,
            "59f410da6d9dad2025f0e53b6c45554a3b3a0a5c574927f201e99d0217c1a26b",
        ),
        (
            "tokenizer_config.json",
            1_412,
            "abda01c8c14c5151ae498aceb30db406d6b91242c394fd88d3b6fd5a63a101e6",
        ),
        (
            "special_tokens_map.json",
            695,
            "5d5b662e421ea9fac075174bb0688ee0d9431699900b90662acd44b2a350503a",
        ),
        (
            "config.json",
            650,
            "1b4d8e2a3988377ed8b519a31d8d31025a25f1c5f8606998e8014111438efcd7",
        ),
    ] {
        hasher.update(path.as_bytes());
        hasher.update(bytes.to_le_bytes());
        hasher.update(hash.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn expected_source_fingerprint(cache: &Connection, commit_id: i64) -> String {
    let (compressed, length): (Vec<u8>, i64) = cache
        .query_row(
            "SELECT message, message_length FROM commits WHERE commit_id = ?1",
            [commit_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let message = lz4_flex::decompress(&compressed, length as usize).unwrap();
    let paths = cache
        .prepare("SELECT raw_path FROM commit_paths WHERE commit_id = ?1 ORDER BY path_order")
        .unwrap()
        .query_map([commit_id], |row| row.get::<_, Vec<u8>>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"gitscry-semantic-source-v1\0");
    hasher.update((message.len() as u64).to_le_bytes());
    hasher.update(&message);
    hasher.update((paths.len() as u64).to_le_bytes());
    for path in paths {
        hasher.update((path.len() as u64).to_le_bytes());
        hasher.update(path);
    }
    format!("{:x}", hasher.finalize())
}

#[test]
fn semantic_enablement_survives_cache_rebuild() {
    let repo = TestRepo::new();
    repo.commit("hello.txt", b"hello\n", "initial commit");
    let missing_runtime_executable = IsolatedExecutable::new();
    let run_with_missing_runtime =
        |args: &[&str]| missing_runtime_executable.run(repo.dir.path(), args);
    assert_eq!(
        run_with_missing_runtime(&["index", "--semantic"])
            .status
            .code(),
        Some(1)
    );
    let cache_path = repo.cache_dir().join("cache.sqlite");
    let cache = Connection::open(&cache_path).unwrap();
    cache
        .execute(
            "UPDATE metadata SET value = '6' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
    drop(cache);

    let rebuilt = run_with_missing_runtime(&["index"]);
    assert_eq!(rebuilt.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&rebuilt.stderr).contains("ordinary history cache is usable"));

    let cache = Connection::open(&cache_path).unwrap();
    let state: (String, String, String, i64) = cache
        .query_row(
            "SELECT
                (SELECT value FROM metadata WHERE key = 'schema_version'),
                (SELECT value FROM metadata WHERE key = 'semantic_enabled'),
                (SELECT value FROM metadata WHERE key = 'semantic_ready'),
                (SELECT COUNT(*) FROM commits)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(state, ("11".to_owned(), "1".to_owned(), "0".to_owned(), 1));
}

#[test]
fn damaged_cache_rebuild_preserves_semantic_enablement() {
    let repo = TestRepo::new();
    repo.commit("hello.txt", b"hello\n", "initial commit");
    let missing_runtime_executable = IsolatedExecutable::new();
    let run_with_missing_runtime =
        |args: &[&str]| missing_runtime_executable.run(repo.dir.path(), args);

    assert_eq!(
        run_with_missing_runtime(&["index", "--semantic"])
            .status
            .code(),
        Some(1)
    );
    let cache_path = repo.cache_dir().join("cache.sqlite");
    let cache = Connection::open(&cache_path).unwrap();
    cache.execute_batch("DROP TABLE semantic_vectors").unwrap();
    drop(cache);

    let rebuilt = run_with_missing_runtime(&["index"]);
    assert_eq!(rebuilt.status.code(), Some(1));
    let error = String::from_utf8_lossy(&rebuilt.stderr);
    assert!(
        error.contains("semantic indexing remains enabled"),
        "{error}"
    );

    let cache = Connection::open(cache_path).unwrap();
    let state: (String, String, String, i64) = cache
        .query_row(
            "SELECT
                (SELECT value FROM metadata WHERE key = 'schema_version'),
                (SELECT value FROM metadata WHERE key = 'semantic_enabled'),
                (SELECT value FROM metadata WHERE key = 'semantic_ready'),
                (SELECT COUNT(*) FROM commits)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(state, ("11".to_owned(), "1".to_owned(), "0".to_owned(), 1));
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
    let cloned = git_command(parent.path())
        .args(["clone", "--depth", "1", "--no-local"])
        .arg(source.dir.path())
        .arg(&clone)
        .output()
        .unwrap();
    assert!(
        cloned.status.success(),
        "{}",
        String::from_utf8_lossy(&cloned.stderr)
    );

    let first = support::isolated_gitscry_command(source.user_data_dir())
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
    let cache_path = git_common_dir(&clone).join("gitscry/cache.sqlite");
    let cache = Connection::open(&cache_path).unwrap();
    assert_eq!(
        cache
            .query_row(
                "SELECT COUNT(*)
                 FROM commit_parents AS p
                 JOIN commits AS c ON c.commit_id = p.commit_id
                 WHERE c.oid = ?1",
                [&merge],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        2
    );
    assert_eq!(
        cache
            .query_row(
                "SELECT COUNT(*)
                 FROM changes AS ch
                 JOIN commits AS c ON c.commit_id = ch.commit_id
                 WHERE c.oid = ?1",
                [&merge],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
    drop(cache);

    git(&clone, ["fetch", "--unshallow"]);
    let second = support::isolated_gitscry_command(source.user_data_dir())
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
        .prepare(
            "SELECT ch.new_path
             FROM changes AS ch
             JOIN commits AS c ON c.commit_id = ch.commit_id
             WHERE c.oid = ?1
             ORDER BY ch.ordinal",
        )
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

    let first = repo.run(["index"]);
    assert_eq!(first.status.code(), Some(0));
    let second = repo.run(["index"]);
    assert_eq!(second.status.code(), Some(0));
    assert_eq!(
        second.stdout,
        b"Indexed 1 commit reachable from current HEAD.\n"
    );
    assert!(second.stderr.is_empty());
}

#[test]
fn fast_forward_updates_one_completed_generation() {
    let repo = TestRepo::new();
    repo.commit("history.txt", b"one\n", "Initial history");
    let first = repo.head();
    assert_eq!(repo.run(["index"]).status.code(), Some(0));

    repo.commit("history.txt", b"two\n", "Second history");
    let second_tip = repo.head();
    let second = repo.run(["index"]);
    assert_eq!(second.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&second.stderr)
            .contains("Indexing history reachable from current HEAD")
    );

    let cache = Connection::open(repo.cache_dir().join("cache.sqlite")).unwrap();
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
                "SELECT COUNT(*)
                 FROM changes AS ch
                 JOIN commits AS c ON c.commit_id = ch.commit_id
                 WHERE c.oid = ?1",
                [&first],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
    assert_eq!(
        cache
            .query_row(
                "SELECT COUNT(*)
                 FROM hunks AS h
                 JOIN changes AS ch ON ch.change_id = h.change_id
                 JOIN commits AS c ON c.commit_id = ch.commit_id
                 WHERE c.oid = ?1",
                [&second_tip],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
}
#[test]
fn file_to_symlink_change_keeps_both_hunks_on_one_change() {
    let repo = TestRepo::new();
    repo.commit("RelNotes", b"release notes\n", "Regular RelNotes file");
    fs::write(
        repo.dir.path().join("symlink-target"),
        b"Documentation/RelNotes/2.3.0.txt",
    )
    .unwrap();
    let target_blob = git_stdout(repo.dir.path(), ["hash-object", "-w", "symlink-target"]);
    fs::remove_file(repo.dir.path().join("symlink-target")).unwrap();
    let cacheinfo = format!("120000,{target_blob},RelNotes");
    git(
        repo.dir.path(),
        ["update-index", "--add", "--cacheinfo", cacheinfo.as_str()],
    );
    git(
        repo.dir.path(),
        ["commit", "-m", "Replace regular RelNotes with symlink"],
    );
    let type_change = repo.head();

    let output = repo.run(["index"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cache = Connection::open(repo.cache_dir().join("cache.sqlite")).unwrap();
    assert_eq!(
        cache
            .query_row(
                "SELECT COUNT(*)
                 FROM changes AS ch
                 JOIN commits AS c ON c.commit_id = ch.commit_id
                 WHERE c.oid = ?1 AND ch.status = 'T'",
                [&type_change],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
    let hunks: Vec<(i64, i64)> = cache
        .prepare(
            "SELECT h.change_id, h.ordinal
             FROM hunks AS h
             JOIN changes AS ch ON ch.change_id = h.change_id
             JOIN commits AS c ON c.commit_id = ch.commit_id
             WHERE c.oid = ?1
             ORDER BY h.ordinal",
        )
        .unwrap()
        .query_map([&type_change], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(hunks.len(), 2);
    assert_eq!(hunks[0].0, hunks[1].0);
    assert_eq!(
        hunks
            .iter()
            .map(|(_, ordinal)| *ordinal)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
}

#[test]
fn multiple_hunks_for_one_change_keep_one_change_id() {
    let repo = TestRepo::new();
    let original = (0..20)
        .map(|line| format!("line {line:02}\n"))
        .collect::<String>();
    repo.commit("history.txt", original.as_bytes(), "Initial history");
    repo.index();

    let changed = original
        .replace("line 01\n", "first replacement\n")
        .replace("line 18\n", "second replacement\n");
    repo.commit("history.txt", changed.as_bytes(), "Two distant edits");
    let target = repo.head();
    repo.index();

    let cache = Connection::open(repo.cache_dir().join("cache.sqlite")).unwrap();
    let change_ids: Vec<i64> = cache
        .prepare(
            "SELECT h.change_id
             FROM hunks AS h
             JOIN changes AS ch ON ch.change_id = h.change_id
             JOIN commits AS c ON c.commit_id = ch.commit_id
             WHERE c.oid = ?1
             ORDER BY h.ordinal",
        )
        .unwrap()
        .query_map([&target], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(change_ids.len(), 2);
    assert_eq!(change_ids[0], change_ids[1]);
}

#[test]
fn indexing_only_current_head_and_reuses_cached_ancestors_across_branches() {
    let repo = TestRepo::new();
    repo.commit("history.txt", b"base\n", "Shared ancestor marker");
    let base = repo.head();
    git(repo.dir.path(), ["branch", "feature"]);

    git(repo.dir.path(), ["checkout", "feature"]);
    repo.commit("feature.txt", b"feature\n", "Feature branch only marker");
    let feature_tip = repo.head();
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

    git(repo.dir.path(), ["checkout", "main"]);
    repo.commit("main.txt", b"main\n", "Main branch only marker");
    let main_tip = repo.head();
    let main_index = repo.run(["index"]);
    assert_eq!(
        main_index.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&main_index.stderr)
    );

    let cache_path = repo.cache_dir().join("cache.sqlite");
    let cache = Connection::open(&cache_path).unwrap();
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
                "SELECT COUNT(*) FROM commits WHERE oid = ?1",
                [&feature_tip],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0,
        "index must ignore the configured default branch when HEAD differs"
    );
    assert_eq!(
        cache
            .query_row(
                "SELECT COUNT(*) FROM commits WHERE oid IN (?1, ?2, ?3)",
                [&base, &main_tip, &feature_tip],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        2
    );
    drop(cache);

    git(repo.dir.path(), ["checkout", "feature"]);
    let feature_index = repo.run(["index"]);
    assert_eq!(
        feature_index.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&feature_index.stderr)
    );
    let progress_output = format!(
        "{}{}",
        String::from_utf8_lossy(&feature_index.stdout),
        String::from_utf8_lossy(&feature_index.stderr)
    );
    assert_eq!(
        progress_output
            .matches("Indexing history reachable from current HEAD...")
            .count(),
        1
    );
    let cache = Connection::open(&cache_path).unwrap();
    assert_eq!(
        cache
            .query_row("SELECT COUNT(*) FROM commits", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        3,
        "indexing the other HEAD should add only its exclusive commit"
    );
    assert_eq!(
        cache
            .query_row(
                "SELECT COUNT(*) FROM commits WHERE oid = ?1",
                [&base],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1,
        "the common ancestor must not be duplicated"
    );
    drop(cache);

    let repeated = repo.run(["index"]);
    assert_eq!(repeated.status.code(), Some(0));
    let cache = Connection::open(&cache_path).unwrap();
    assert_eq!(
        cache
            .query_row("SELECT COUNT(*) FROM commits", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        3,
        "re-indexing must not duplicate shared material"
    );
    drop(cache);
    let feature_present_search = repo.run(["search", "Feature branch only marker"]);
    assert_eq!(feature_present_search.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&feature_present_search.stdout)
            .contains("Feature branch only marker")
    );

    git(repo.dir.path(), ["checkout", "main"]);
    let feature_search = repo.run(["search", "Feature branch only marker"]);
    assert_eq!(feature_search.status.code(), Some(0));
    assert!(
        !String::from_utf8_lossy(&feature_search.stdout).contains("Feature branch only marker")
    );
    let main_search = repo.run(["search", "Main branch only marker"]);
    assert_eq!(main_search.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&main_search.stdout).contains("Main branch only marker"));
    git(repo.dir.path(), ["checkout", "--detach", &base]);
    let detached_index = repo.run(["index"]);
    assert_eq!(
        detached_index.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&detached_index.stderr)
    );
    let cache = Connection::open(&cache_path).unwrap();
    let completed_tip: String = cache
        .query_row(
            "SELECT value FROM metadata WHERE key = 'completed_tip'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(completed_tip, base);
    assert_eq!(
        cache
            .query_row("SELECT COUNT(*) FROM commits", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        3,
        "detached indexing must reuse the same shared material"
    );
    drop(cache);
    let detached_search = repo.run(["search", "Shared ancestor marker"]);
    assert_eq!(detached_search.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&detached_search.stdout).contains("Shared ancestor marker"));
    let excluded_search = repo.run(["search", "Main branch only marker"]);
    assert_eq!(excluded_search.status.code(), Some(0));
    assert!(!String::from_utf8_lossy(&excluded_search.stdout).contains("Main branch only marker"));
}

#[test]
fn indexing_after_rewrite_preserves_previously_cached_history() {
    let repo = TestRepo::new();
    repo.commit("history.txt", b"one\n", "Initial history");
    let base = repo.head();
    repo.commit("history.txt", b"two\n", "Old second history");
    let old_tip = repo.head();
    git(repo.dir.path(), ["branch", "old-history"]);
    assert_eq!(repo.run(["index"]).status.code(), Some(0));

    git(repo.dir.path(), ["reset", "--hard", &base]);
    repo.commit("history.txt", b"replacement\n", "Replacement history");
    let new_tip = repo.head();
    let output = repo.run(["index"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    git(repo.dir.path(), ["branch", "-D", "old-history"]);
    assert_eq!(repo.run(["index"]).status.code(), Some(0));

    let cache = Connection::open(repo.cache_dir().join("cache.sqlite")).unwrap();
    for oid in [&base, &old_tip, &new_tip] {
        assert_eq!(
            cache
                .query_row(
                    "SELECT COUNT(*) FROM commits WHERE oid = ?1",
                    [oid],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1,
            "cached commit {oid} must survive a history rewrite and branch deletion"
        );
    }
    let total: i64 = cache
        .query_row("SELECT COUNT(*) FROM commits", [], |row| row.get(0))
        .unwrap();
    assert_eq!(total, 3);
    let old_hunks: i64 = cache
        .query_row(
            "SELECT COUNT(*) FROM hunks AS h
             JOIN changes AS ch ON ch.change_id = h.change_id
             JOIN commits AS c ON c.commit_id = ch.commit_id
             WHERE c.oid = ?1",
            [&old_tip],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(old_hunks, 1);
}

#[test]
fn indexing_expanded_shallow_history_links_new_ancestors_without_rebuilding() {
    let repo = TestRepo::new();
    repo.commit("history.txt", b"one\n", "Shallow ancestor marker");
    let ancestor = repo.head();
    repo.commit("history.txt", b"two\n", "Shallow boundary marker");
    let boundary = repo.head();
    fs::write(
        repo.dir.path().join(".git/shallow"),
        format!("{boundary}\n"),
    )
    .expect("make the repository shallow at its tip");
    git(repo.dir.path(), ["checkout", "-b", "feature"]);
    repo.commit("feature.txt", b"feature\n", "Shallow feature marker");
    let feature_tip = repo.head();
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
    let shallow_index = repo.run(["index"]);
    assert_eq!(
        shallow_index.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&shallow_index.stderr)
    );

    git(repo.dir.path(), ["checkout", "main"]);
    fs::remove_file(repo.dir.path().join(".git/shallow")).expect("unshallow the repository");
    git(
        repo.dir.path(),
        ["update-ref", "refs/remotes/origin/main", &boundary],
    );
    git(
        repo.dir.path(),
        [
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    let expanded_index = repo.run(["index"]);
    assert_eq!(
        expanded_index.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&expanded_index.stderr)
    );

    let cache = Connection::open(repo.cache_dir().join("cache.sqlite")).unwrap();
    assert_eq!(
        cache
            .query_row(
                "SELECT COUNT(*) FROM commits WHERE oid = ?1",
                [&feature_tip],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1,
        "expanding history must preserve material indexed from another HEAD"
    );
    let linked_parent: (Option<String>, Option<String>) = cache
        .query_row(
            "SELECT parent.oid, p.external_oid
             FROM commit_parents AS p
             JOIN commits AS child ON child.commit_id = p.commit_id
             LEFT JOIN commits AS parent ON parent.commit_id = p.parent_id
             WHERE child.oid = ?1 AND p.position = 0",
            [&boundary],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(linked_parent, (Some(ancestor), None));

    let ancestor_search = repo.run(["search", "Shallow ancestor marker"]);
    assert_eq!(ancestor_search.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&ancestor_search.stdout).contains("Shallow ancestor marker"));
    let other_head_search = repo.run(["search", "Shallow feature marker"]);
    assert_eq!(other_head_search.status.code(), Some(0));
    assert!(!String::from_utf8_lossy(&other_head_search.stdout).contains("Shallow feature marker"));
}

#[test]
fn restores_hunks_after_a_missing_blob_returns() {
    let repo = TestRepo::new();
    repo.commit("history.txt", b"one\n", "First history");
    let first = repo.head();
    repo.commit("history.txt", b"two\n", "Second history");
    repo.commit("history.txt", b"three\n", "Third history");
    let third = repo.head();

    let blob = git_stdout(repo.dir.path(), ["ls-tree", &first, "history.txt"]);
    let blob = blob.split_whitespace().nth(2).unwrap().to_owned();
    let object_path = repo
        .dir
        .path()
        .join(".git/objects")
        .join(&blob[..2])
        .join(&blob[2..]);
    fs::remove_file(object_path).expect("remove loose blob");

    let first_index = repo.run(["index"]);
    assert_eq!(first_index.status.code(), Some(0));
    let cache_path = repo.cache_dir().join("cache.sqlite");
    let cache = Connection::open(&cache_path).unwrap();
    let incomplete_hunk_count: i64 = cache
        .query_row("SELECT COUNT(*) FROM hunks", [], |row| row.get(0))
        .unwrap();
    assert_eq!(incomplete_hunk_count, 1);
    let incomplete_hunk_commit: String = cache
        .query_row(
            "SELECT c.oid
             FROM hunks AS h
             JOIN changes AS ch ON ch.change_id = h.change_id
             JOIN commits AS c ON c.commit_id = ch.commit_id",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(incomplete_hunk_commit, third);
    drop(cache);

    fs::write(repo.dir.path().join("restore.txt"), b"one\n").unwrap();
    git(repo.dir.path(), ["hash-object", "-w", "restore.txt"]);
    fs::remove_file(repo.dir.path().join("restore.txt")).unwrap();

    let restored = repo.run(["index"]);
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
    assert_eq!(repo.run(["index"]).status.code(), Some(0));
    let cache_path = repo.cache_dir().join("cache.sqlite");
    let completed_tip = repo.head();

    let object_path = repo
        .dir
        .path()
        .join(".git/objects")
        .join(&missing_commit[..2])
        .join(&missing_commit[2..]);
    fs::remove_file(object_path).expect("remove loose commit");

    let output = repo.run(["index"]);
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
    if env::var("GITSCRY_LOCK_SHARED").ok().as_deref() == Some("1") {
        lock.lock_shared().expect("shared cache lock");
    } else {
        lock.lock().expect("exclusive cache lock");
    }
    fs::write(ready_path, b"ready").expect("signal lock holder");
    let hold_millis = env::var("GITSCRY_LOCK_HOLD_MILLIS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1_000);
    thread::sleep(Duration::from_millis(hold_millis));
}

fn spawn_cache_lock_holder(
    repo: &TestRepo,
    ready_name: &str,
    shared: bool,
    hold_millis: u64,
) -> std::process::Child {
    let ready_path = repo.dir.path().join(ready_name);
    let mut command = Command::new(env::current_exe().unwrap());
    command
        .args(["--exact", "cache_lock_holder", "--nocapture"])
        .current_dir(repo.dir.path())
        .env("GITSCRY_LOCK_HOLDER", "1")
        .env("GITSCRY_LOCK_SHARED", if shared { "1" } else { "0" })
        .env("GITSCRY_LOCK_PATH", repo.cache_dir().join("cache.lock"))
        .env("GITSCRY_LOCK_READY", &ready_path)
        .env("GITSCRY_LOCK_HOLD_MILLIS", hold_millis.to_string());
    let holder = command.spawn().expect("spawn lock holder");
    for _ in 0..200 {
        if ready_path.exists() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(ready_path.exists(), "lock holder did not start");
    holder
}

#[test]
fn stale_observer_waits_once_for_an_existing_writer() {
    let repo = TestRepo::new();
    repo.commit("history.txt", b"one\n", "Initial history");
    assert_eq!(repo.run(["index"]).status.code(), Some(0));

    let ready_path = repo.dir.path().join("lock-ready");
    let holder = Command::new(env::current_exe().unwrap())
        .args(["--exact", "cache_lock_holder", "--nocapture"])
        .current_dir(repo.dir.path())
        .env("GITSCRY_LOCK_HOLDER", "1")
        .env("GITSCRY_LOCK_SHARED", "0")
        .env("GITSCRY_LOCK_PATH", repo.cache_dir().join("cache.lock"))
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

    let output = repo.run(["index"]);
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
    assert_eq!(repo.run(["index"]).status.code(), Some(0));

    let cache = repo.cache_dir().join("cache.sqlite");
    let previous = repo.cache_dir().join("cache.sqlite.previous");
    fs::rename(&cache, &previous).expect("simulate interrupted replacement");

    let output = repo.run(["index"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        output.stdout,
        b"Indexed 1 commit reachable from current HEAD.\n"
    );
    assert!(output.stderr.is_empty());
    assert!(cache.is_file());
    assert!(!previous.exists());
}

#[test]
fn concurrent_indexers_on_distinct_heads_preserve_both_histories() {
    let repo = TestRepo::new();
    repo.commit("base.txt", b"base\n", "Shared ancestor");
    let base = repo.head();
    let linked_root = tempfile::tempdir().expect("create linked worktree parent");
    let linked = linked_root.path().join("feature");
    git(
        repo.dir.path(),
        [
            "worktree",
            "add",
            "--detach",
            linked.to_str().expect("linked worktree path"),
            &base,
        ],
    );

    for index in 0..12 {
        let contents = format!("main {index}\n");
        let message = format!("main commit {index}");
        repo.commit("main.txt", contents.as_bytes(), &message);

        let contents = format!("feature {index}\n");
        let message = format!("feature commit {index}");
        fs::write(linked.join("feature.txt"), contents).expect("write feature history");
        git(&linked, ["add", "feature.txt"]);
        git(&linked, ["commit", "-m", &message]);
    }
    let main_tip = repo.head();
    let feature_tip = git_stdout(&linked, ["rev-parse", "HEAD"]);

    let first = support::isolated_gitscry_command(repo.user_data_dir())
        .arg("index")
        .current_dir(repo.dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let second = support::isolated_gitscry_command(repo.user_data_dir())
        .arg("index")
        .current_dir(&linked)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let first = first.wait_with_output().unwrap();
    let second = second.wait_with_output().unwrap();

    assert_eq!(
        first.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(
        second.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&first.stdout),
        "Indexed 13 commits reachable from current HEAD.\n",
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&second.stdout),
        "Indexed 13 commits reachable from current HEAD.\n",
        "stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let cache = Connection::open(repo.cache_dir().join("cache.sqlite")).unwrap();
    assert_eq!(
        cache
            .query_row("SELECT COUNT(*) FROM commits", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        25,
        "the cache should retain the shared base and both branch histories"
    );
    assert_eq!(
        cache
            .query_row(
                "SELECT COUNT(*) FROM commits WHERE oid IN (?1, ?2)",
                [&main_tip, &feature_tip],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        2
    );
}

#[test]
fn plain_index_requires_explicit_semantic_encoder_migration() {
    let repo = TestRepo::new();
    repo.commit("one.txt", b"one\n", "initial commit");
    assert!(repo.run(["index"]).status.success());

    let cache_path = repo.cache_dir().join("cache.sqlite");
    let cache = Connection::open(&cache_path).unwrap();
    let (commit_id, oid): (i64, String) = cache
        .query_row("SELECT commit_id, oid FROM commits", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    let source_fingerprint = expected_source_fingerprint(&cache, commit_id);
    let embedding = normalized_vector(0);
    let stale_fingerprint = "d".repeat(64);
    let tip = repo.head();
    for (key, value) in [
        ("semantic_enabled", "1"),
        ("semantic_ready", "1"),
        ("semantic_coverage_tip", tip.as_str()),
        ("semantic_coverage_count", "1"),
        ("semantic_encoder_fingerprint", stale_fingerprint.as_str()),
        ("semantic_runtime_provenance", "previous-runtime"),
    ] {
        cache
            .execute(
                "UPDATE metadata SET value = ?1 WHERE key = ?2",
                [value, key],
            )
            .unwrap();
    }
    cache
        .execute(
            "INSERT INTO semantic_vectors(
                commit_id, commit_oid, embedding, source_fingerprint, input_fingerprint,
                encoder_fingerprint, runtime_provenance
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                commit_id,
                oid,
                embedding,
                source_fingerprint,
                "c".repeat(64),
                stale_fingerprint,
                "previous-runtime"
            ],
        )
        .unwrap();
    drop(cache);

    let executable = IsolatedExecutable::new();
    let run = |args: &[&str]| executable.run(repo.dir.path(), args);
    let plain = run(&["index"]);
    assert_eq!(plain.status.code(), Some(1));
    let error = String::from_utf8_lossy(&plain.stderr);
    assert!(
        error.contains("ordinary history cache is usable"),
        "{error}"
    );
    assert!(error.contains("gitscry index --semantic"), "{error}");
    assert!(repo.run(["search", "initial"]).status.success());

    let explicit = run(&["index", "--semantic"]);
    assert_eq!(explicit.status.code(), Some(1));
    let error = String::from_utf8_lossy(&explicit.stderr);
    assert!(
        error.contains("reinstall GitScry from an official release"),
        "{error}"
    );
    let cache = Connection::open(cache_path).unwrap();
    let (enabled, ready, count): (String, String, i64) = cache
        .query_row(
            "SELECT
                (SELECT value FROM metadata WHERE key = 'semantic_enabled'),
                (SELECT value FROM metadata WHERE key = 'semantic_ready'),
                (SELECT COUNT(*) FROM semantic_vectors)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!((enabled.as_str(), ready.as_str(), count), ("1", "0", 1));
}

#[test]
fn reduced_shallow_history_retains_cached_rows() {
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
    let cloned = git_command(parent.path())
        .args(["clone", "--depth", "2", "--no-local"])
        .arg(source.dir.path())
        .arg(&clone)
        .output()
        .unwrap();
    assert!(cloned.status.success());

    let first = support::isolated_gitscry_command(source.user_data_dir())
        .arg("index")
        .current_dir(&clone)
        .output()
        .unwrap();
    assert_eq!(first.status.code(), Some(0));
    let cache_path = git_common_dir(&clone).join("gitscry/cache.sqlite");
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
    let second = support::isolated_gitscry_command(source.user_data_dir())
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
        2
    );
}

#[test]
fn path_projection_preserves_rename_paths_and_commit_counts() {
    let repo = TestRepo::new();
    repo.commit("old.rs", b"old\n", "old");
    repo.commit("old.rs", b"changed\n", "modify");
    let modify_oid = repo.head();
    git(repo.dir.path(), ["mv", "old.rs", "new.rs"]);
    git(repo.dir.path(), ["commit", "-m", "rename"]);
    repo.index();

    let cache = Connection::open(repo.cache_dir().join("cache.sqlite")).unwrap();
    let rename_oid = repo.head();
    let paths: Vec<(String, Vec<u8>)> = cache
        .prepare(
            "SELECT cp.path_search_key, cp.raw_path
             FROM commit_paths AS cp
             JOIN commits AS c ON c.commit_id = cp.commit_id
             WHERE c.oid = ?1
             ORDER BY cp.path_order",
        )
        .unwrap()
        .query_map([&rename_oid], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    assert_eq!(
        paths,
        vec![
            ("old.rs".to_owned(), b"old.rs".to_vec()),
            ("new.rs".to_owned(), b"new.rs".to_vec()),
        ]
    );
    assert_eq!(
        cache
            .query_row(
                "SELECT path_count
                 FROM commit_path_counts AS counts
                 JOIN commits AS c ON c.commit_id = counts.commit_id
                 WHERE c.oid = ?1",
                [&modify_oid],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
    assert_eq!(
        cache
            .query_row(
                "SELECT path_count
                 FROM commit_path_counts AS counts
                 JOIN commits AS c ON c.commit_id = counts.commit_id
                 WHERE c.oid = ?1",
                [&rename_oid],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        2
    );
}

#[test]
fn linked_worktrees_share_the_repository_cache_and_lock() {
    let repo = TestRepo::new();
    repo.commit("history.txt", b"shared\n", "Initial shared worktree marker");
    repo.index();

    let linked = repo.dir.path().join("linked");
    let linked_arg = linked.to_string_lossy().into_owned();
    git(
        repo.dir.path(),
        ["worktree", "add", "--detach", &linked_arg, "HEAD"],
    );
    let cache_path = repo.cache_dir().join("cache.sqlite");
    assert!(
        cache_path.is_file(),
        "cache belongs in the shared Git directory"
    );
    assert_eq!(
        PathBuf::from(git_stdout(
            &linked,
            ["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )),
        repo.common_dir()
    );
    assert!(!repo.dir.path().join(".gitscry").exists());
    assert!(!repo.dir.path().join("gitscry").exists());
    assert!(!linked.join(".gitscry").exists());
    assert!(!linked.join("gitscry").exists());

    let query = TestRepo::run_at(&linked, ["search", "shared", "worktree", "marker"]);
    assert_eq!(
        query.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&query.stderr)
    );
    assert!(String::from_utf8_lossy(&query.stdout).contains("Initial shared worktree marker"));

    let exclusive_holder = spawn_cache_lock_holder(&repo, "exclusive-lock-ready", false, 1_000);
    let cache_path = repo.cache_dir().join("cache.sqlite");
    let unpublished_cache_path = repo.cache_dir().join("cache.sqlite.unpublished");
    fs::rename(&cache_path, &unpublished_cache_path).expect("temporarily unpublish cache");
    let query_root = linked.clone();
    let blocked_query = thread::spawn(move || {
        TestRepo::run_at(&query_root, ["search", "shared", "worktree", "marker"])
    });
    thread::sleep(Duration::from_millis(250));
    let query_returned_while_cache_was_unpublished = blocked_query.is_finished();
    fs::rename(&unpublished_cache_path, &cache_path).expect("republish cache");
    let _ = exclusive_holder
        .wait_with_output()
        .expect("wait for exclusive lock holder");
    let blocked_query = blocked_query.join().expect("wait for query");
    assert!(
        !query_returned_while_cache_was_unpublished,
        "query must wait for the writer when publication temporarily unpublishes the cache"
    );
    assert_eq!(blocked_query.status.code(), Some(0));
    let query_output = format!(
        "{}{}",
        String::from_utf8_lossy(&blocked_query.stdout),
        String::from_utf8_lossy(&blocked_query.stderr)
    );
    assert_eq!(
        query_output
            .matches("Waiting for another GitScry process...")
            .count(),
        1
    );

    fs::write(linked.join("history.txt"), b"updated\n").unwrap();
    git(&linked, ["add", "history.txt"]);
    git(&linked, ["commit", "-m", "Update shared worktree history"]);

    let shared_holder = spawn_cache_lock_holder(&repo, "shared-lock-ready", true, 5_000);
    let shared_query = TestRepo::run_at(&linked, ["search", "shared", "worktree", "marker"]);
    assert_eq!(shared_query.status.code(), Some(0));
    let shared_query_output = format!(
        "{}{}",
        String::from_utf8_lossy(&shared_query.stdout),
        String::from_utf8_lossy(&shared_query.stderr)
    );
    assert!(shared_query_output.contains("Waiting for another GitScry process..."));
    let _ = shared_holder
        .wait_with_output()
        .expect("wait for shared lock holder");
    fs::write(linked.join("history.txt"), b"updated again\n").unwrap();
    git(&linked, ["add", "history.txt"]);
    git(
        &linked,
        ["commit", "-m", "Update history after query refresh"],
    );
    let shared_holder = spawn_cache_lock_holder(&repo, "index-lock-ready", true, 5_000);
    let blocked_index = TestRepo::run_at(&linked, ["index"]);
    let _ = shared_holder
        .wait_with_output()
        .expect("wait for shared lock holder");
    assert_eq!(
        blocked_index.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&blocked_index.stderr)
    );
    let index_output = format!(
        "{}{}",
        String::from_utf8_lossy(&blocked_index.stdout),
        String::from_utf8_lossy(&blocked_index.stderr)
    );
    assert_eq!(
        index_output
            .matches("Waiting for another GitScry process...")
            .count(),
        1
    );
    let cache = Connection::open(&cache_path).unwrap();
    assert_eq!(
        cache
            .query_row("SELECT COUNT(*) FROM commits", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert!(!repo.dir.path().join(".gitscry").exists());
    assert!(!linked.join(".gitscry").exists());
}
