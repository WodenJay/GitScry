mod support;

use std::{
    env,
    fs::{self, OpenOptions},
    path::Path,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use support::{TestRepo, git, git_command};

fn commit(repo: &TestRepo, contents: &str, subject: &str) {
    commit_file(repo, "history.txt", contents, subject);
}

fn blob_oid(repo: &TestRepo, commit_oid: &str, path: &str) -> String {
    let tree_path = format!("{commit_oid}:{path}");
    let output = git_command(repo.dir.path())
        .args(["rev-parse", tree_path.as_str()])
        .output()
        .expect("read blob ID");
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn commit_file(repo: &TestRepo, path: &str, contents: &str, subject: &str) {
    fs::write(repo.dir.path().join(path), contents).expect("write history");
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

#[test]
fn prune_dry_run_without_cache_creates_no_cache_data() {
    let repo = TestRepo::new();
    let cache = repo.cache_dir();
    assert!(!cache.exists());

    let output = repo.run(["prune", "--dry-run"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "prune preview failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("candidate commits: 0"), "{stdout}");
    assert!(stdout.contains("before: 0 bytes"), "{stdout}");
    assert!(!cache.join("cache.sqlite").exists());
    let mut entries = fs::read_dir(cache)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    entries.sort();
    assert_eq!(entries, ["cache.lock"]);
}

#[test]
fn prune_help_explains_git_object_retention_and_dry_run_limits() {
    let repo = TestRepo::new();
    let output = repo.run(["prune", "--help"]);
    assert_eq!(output.status.code(), Some(0));
    let help = String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(help.contains("repository-wide"), "{help}");
    assert!(help.contains("Git still retains"), "{help}");
    assert!(help.contains("--dry-run"), "{help}");
    assert!(help.contains("does not estimate reclaimed bytes"), "{help}");
    assert!(help.contains("deleted commits"), "{help}");
    assert!(help.contains("semantic vectors"), "{help}");
    assert!(
        help.contains("measured before/after cache data-file sizes"),
        "{help}"
    );
    assert!(help.contains("bytes actually released"), "{help}");
    assert!(help.contains("physical filesystem allocation"), "{help}");
}

#[test]
fn prune_dry_run_reports_candidates_and_preserves_cache_data() {
    let repo = TestRepo::new();
    commit(&repo, "retained history\n", "Retained history");
    repo.index();

    let before = cache_snapshot(&repo.cache_dir());
    let before_bytes = before
        .iter()
        .filter(|(name, _)| name != "cache.lock" && name != ".gitignore")
        .map(|(_, data)| data.len())
        .sum::<usize>();
    let output = repo.run(["prune", "--dry-run"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "prune --dry-run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("candidate commits: 0"), "{stdout}");
    assert!(
        stdout.contains(&format!("before: {before_bytes} bytes")),
        "{stdout}"
    );
    assert!(
        stdout.contains("does not estimate reclaimed bytes"),
        "{stdout}"
    );
    assert_eq!(cache_snapshot(&repo.cache_dir()), before);
}

#[test]
fn prune_removes_only_missing_objects_and_preserves_surviving_history() {
    let repo = TestRepo::new();
    commit(&repo, "base history\n", "Base history");
    git(repo.dir.path(), ["switch", "-c", "side"]);
    fs::write(
        repo.dir.path().join("candidate-only.txt"),
        "candidate-only payload\n",
    )
    .expect("write candidate-only path");
    commit(&repo, "branch-only history\n", "Branch object to prune");
    let side_oid = repo.head();
    let missing_blob_oid = blob_oid(&repo, &side_oid, "candidate-only.txt");
    let shared_blob_oid = blob_oid(&repo, &side_oid, "history.txt");
    commit_file(
        &repo,
        "branch-child.txt",
        "branch child retained\n",
        "Branch child retained",
    );
    let side_child_oid = repo.head();
    repo.index();

    git(repo.dir.path(), ["switch", "main"]);
    commit(&repo, "branch-only history\n", "Surviving history marker");
    repo.index();
    git(repo.dir.path(), ["branch", "-D", "side"]);
    assert!(
        git_command(repo.dir.path())
            .args(["cat-file", "-e", &side_oid])
            .status()
            .expect("check retained Git object")
            .success()
    );

    let before = cache_snapshot(&repo.cache_dir());
    let retained = repo.run(["prune"]);
    assert_eq!(retained.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&retained.stdout).contains("deleted commits: 0"));
    assert_eq!(cache_snapshot(&repo.cache_dir()), before);

    let vector_connection = rusqlite::Connection::open(repo.cache_dir().join("cache.sqlite"))
        .expect("open indexed cache");
    let fingerprint = "f".repeat(64);
    vector_connection
        .execute(
            "INSERT INTO semantic_vectors
                (commit_id, commit_oid, embedding, source_fingerprint,
                 input_fingerprint, encoder_fingerprint, runtime_provenance)
             SELECT commit_id, oid, zeroblob(1536), ?2, ?2, ?2, 'prune test'
             FROM commits WHERE oid = ?1",
            rusqlite::params![side_oid, fingerprint],
        )
        .expect("add candidate vector");
    drop(vector_connection);

    for oid in [&missing_blob_oid, &shared_blob_oid] {
        let blob_path = repo
            .common_dir()
            .join("objects")
            .join(&oid[..2])
            .join(&oid[2..]);
        assert!(blob_path.is_file(), "expected candidate blob object");
        fs::remove_file(blob_path).expect("remove candidate blob object");
    }
    let cache_connection =
        rusqlite::Connection::open(repo.cache_dir().join("cache.sqlite")).unwrap();
    for oid in [&missing_blob_oid, &shared_blob_oid] {
        cache_connection
            .execute("INSERT INTO missing_objects(oid) VALUES (?1)", [oid])
            .unwrap();
    }
    drop(cache_connection);

    let object_path = repo
        .common_dir()
        .join("objects")
        .join(&side_oid[..2])
        .join(&side_oid[2..]);
    assert!(object_path.is_file(), "expected loose commit object");
    fs::remove_file(object_path).expect("remove side-branch commit object");
    let missing = git_command(repo.dir.path())
        .args(["cat-file", "-e", &side_oid])
        .status()
        .expect("check removed Git object");
    assert!(!missing.success());

    let before_preview = cache_snapshot(&repo.cache_dir());
    let preview = repo.run(["prune", "--dry-run"]);
    assert_eq!(
        preview.status.code(),
        Some(0),
        "prune preview failed: {}",
        String::from_utf8_lossy(&preview.stderr)
    );
    let preview_stdout = String::from_utf8_lossy(&preview.stdout);
    for expected in [
        "candidate commits: 1",
        "changes: 2",
        "path records: 2",
        "hunks: 2",
        "semantic vectors: 1",
    ] {
        assert!(
            preview_stdout.contains(expected),
            "missing {expected}: {preview_stdout}"
        );
    }
    assert_eq!(cache_snapshot(&repo.cache_dir()), before_preview);

    let pruned = repo.run(["prune"]);
    assert_eq!(
        pruned.status.code(),
        Some(0),
        "prune failed: {}",
        String::from_utf8_lossy(&pruned.stderr)
    );
    let stdout = String::from_utf8_lossy(&pruned.stdout);
    for expected in [
        "deleted commits: 1",
        "changes: 2",
        "path records: 2",
        "hunks: 2",
        "semantic vectors: 1",
        "after:",
        "released:",
    ] {
        assert!(stdout.contains(expected), "missing {expected}: {stdout}");
    }

    let remaining_vectors: i64 = rusqlite::Connection::open(repo.cache_dir().join("cache.sqlite"))
        .unwrap()
        .query_row("SELECT COUNT(*) FROM semantic_vectors", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(remaining_vectors, 0);

    let graph_connection =
        rusqlite::Connection::open(repo.cache_dir().join("cache.sqlite")).unwrap();
    let candidate_blob_remains: bool = graph_connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM missing_objects WHERE oid = ?1)",
            [&missing_blob_oid],
            |row| row.get(0),
        )
        .unwrap();
    let shared_blob_remains: bool = graph_connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM missing_objects WHERE oid = ?1)",
            [&shared_blob_oid],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        !candidate_blob_remains,
        "prune retained a missing blob used only by a removed commit"
    );
    assert!(
        shared_blob_remains,
        "prune removed a missing blob used by a surviving commit"
    );
    let external_parent: (Option<i64>, Option<String>) = graph_connection
        .query_row(
            "SELECT parent_id, external_oid FROM commit_parents
             WHERE commit_id = (SELECT commit_id FROM commits WHERE oid = ?1)
               AND position = 0",
            [&side_child_oid],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(external_parent, (None, Some(side_oid)));
    let query = repo.run(["search", "Surviving history marker"]);
    assert_eq!(
        query.status.code(),
        Some(0),
        "surviving history query failed: {}",
        String::from_utf8_lossy(&query.stderr)
    );
    assert!(String::from_utf8_lossy(&query.stdout).contains("Surviving history marker"));
    let code_query = repo.run(["search", "--code", "branch-only history"]);
    assert_eq!(
        code_query.status.code(),
        Some(0),
        "surviving hunk query failed: {}",
        String::from_utf8_lossy(&code_query.stderr)
    );
    assert!(String::from_utf8_lossy(&code_query.stdout).contains("branch-only history"));
}

#[test]
fn git_object_check_failure_does_not_delete_cache_material() {
    let repo = TestRepo::new();
    commit(&repo, "first state\n", "First retained commit");
    commit(&repo, "second state\n", "Commit to corrupt");
    let corrupt_oid = repo.head();
    commit(&repo, "third state\n", "Latest retained commit");
    repo.index();

    let object_path = repo
        .common_dir()
        .join("objects")
        .join(&corrupt_oid[..2])
        .join(&corrupt_oid[2..]);
    assert!(object_path.is_file(), "expected loose commit object");
    fs::remove_file(&object_path).expect("remove commit object before corruption");
    fs::write(object_path, b"not a Git object").expect("corrupt commit object");

    let before = cache_snapshot(&repo.cache_dir());
    let output = repo.run(["prune"]);
    assert_ne!(output.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Git object check failed"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(cache_snapshot(&repo.cache_dir()), before);
    let still_cached: bool = rusqlite::Connection::open(repo.cache_dir().join("cache.sqlite"))
        .unwrap()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM commits WHERE oid = ?1)",
            [&corrupt_oid],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        still_cached,
        "failed Git inspection deleted cached commit material"
    );
}

#[test]
fn prune_waits_for_shared_and_exclusive_locks_across_worktrees() {
    let repo = TestRepo::new();
    commit(
        &repo,
        "lock coordination marker\n",
        "Prune lock coordination marker",
    );
    repo.index();

    let worktree_parent = tempfile::tempdir().expect("create worktree parent");
    let linked = worktree_parent.path().join("linked");
    git(
        repo.dir.path(),
        [
            "worktree",
            "add",
            "--detach",
            linked.to_str().expect("worktree path is UTF-8"),
            "HEAD",
        ],
    );
    let cache_lock = repo.cache_dir().join("cache.lock");
    assert!(cache_lock.is_file());

    for shared in [true, false] {
        let ready_path = repo.dir.path().join(if shared {
            "shared-ready"
        } else {
            "exclusive-ready"
        });
        let holder = Command::new(env::current_exe().unwrap())
            .args(["--exact", "prune_lock_holder", "--nocapture"])
            .current_dir(repo.dir.path())
            .env("GITSCRY_PRUNE_LOCK_HOLDER", "1")
            .env("GITSCRY_PRUNE_LOCK_SHARED", if shared { "1" } else { "0" })
            .env("GITSCRY_PRUNE_LOCK_PATH", &cache_lock)
            .env("GITSCRY_PRUNE_LOCK_READY", &ready_path)
            .env("GITSCRY_PRUNE_LOCK_HOLD_MILLIS", "1000")
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

        let mut prune = TestRepo::command_at(&linked, repo.user_data_dir())
            .args(["prune", "--dry-run"])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        thread::sleep(Duration::from_millis(70));
        assert!(
            prune.try_wait().unwrap().is_none(),
            "prune skipped the cache lock"
        );
        let output = prune.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("Waiting for another GitScry process")
        );
        assert!(holder.wait_with_output().unwrap().status.success());
    }
    drop(worktree_parent);
}

#[test]
fn prune_lock_holder() {
    if env::var_os("GITSCRY_PRUNE_LOCK_HOLDER").is_none() {
        return;
    }
    let lock_path = env::var_os("GITSCRY_PRUNE_LOCK_PATH").expect("cache lock path");
    let ready_path = env::var_os("GITSCRY_PRUNE_LOCK_READY").expect("lock ready path");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(lock_path)
        .expect("open cache lock");
    if env::var("GITSCRY_PRUNE_LOCK_SHARED").ok().as_deref() == Some("1") {
        lock.lock_shared().expect("hold shared cache lock");
    } else {
        lock.lock().expect("hold exclusive cache lock");
    }
    fs::write(ready_path, b"ready").expect("signal lock holder");
    let millis = env::var("GITSCRY_PRUNE_LOCK_HOLD_MILLIS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1000);
    thread::sleep(Duration::from_millis(millis));
}

#[test]
fn indexing_restores_pruned_commit_objects_in_history_order() {
    let repo = TestRepo::new();
    commit(&repo, "first state\n", "First history commit");
    let first = repo.head();
    repo.index();
    commit(&repo, "second state\n", "Second history commit");
    let restored = repo.head();
    repo.index();
    git(
        repo.dir.path(),
        ["switch", "-c", "unrelated", first.as_str()],
    );
    commit(&repo, "unrelated state\n", "Unrelated branch history");
    let unrelated = repo.head();
    repo.index();
    git(repo.dir.path(), ["switch", "main"]);
    commit(&repo, "third state\n", "Third history commit");
    let tip = repo.head();
    repo.index();
    let object_path = repo
        .common_dir()
        .join("objects")
        .join(&restored[..2])
        .join(&restored[2..]);
    let object = fs::read(&object_path).expect("read commit object before removal");
    fs::remove_file(&object_path).expect("remove commit object before prune");

    let pruned = repo.run(["prune"]);
    assert_eq!(
        pruned.status.code(),
        Some(0),
        "prune failed: {}",
        String::from_utf8_lossy(&pruned.stderr)
    );
    assert!(String::from_utf8_lossy(&pruned.stdout).contains("deleted commits: 1"));

    let cache_path = repo.cache_dir().join("cache.sqlite");
    let cache = rusqlite::Connection::open(&cache_path).expect("open pruned cache");
    let restored_commit_is_absent: bool = cache
        .query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM commits WHERE oid = ?1)",
            [&restored],
            |row| row.get(0),
        )
        .expect("check pruned commit");
    assert!(
        restored_commit_is_absent,
        "prune retained unavailable commit"
    );
    let pruned_child_parent: (Option<i64>, Option<String>) = cache
        .query_row(
            "SELECT parent.parent_id, parent.external_oid
             FROM commit_parents AS parent
             JOIN commits AS child ON child.commit_id = parent.commit_id
             WHERE child.oid = ?1 AND parent.position = 0",
            [&tip],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read surviving child's unresolved parent");
    assert_eq!(pruned_child_parent, (None, Some(restored.clone())));
    drop(cache);

    fs::create_dir_all(object_path.parent().unwrap()).expect("restore object directory");
    fs::write(&object_path, object).expect("restore exact commit object");
    let indexed = repo.run(["index"]);
    assert_eq!(
        indexed.status.code(),
        Some(0),
        "index failed: {}",
        String::from_utf8_lossy(&indexed.stderr)
    );

    let cache = rusqlite::Connection::open(cache_path).expect("open repaired cache");
    let unrelated_commit_is_cached: bool = cache
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM commits WHERE oid = ?1)",
            [&unrelated],
            |row| row.get(0),
        )
        .expect("check unrelated branch history");
    assert!(
        unrelated_commit_is_cached,
        "index discarded unrelated cached history"
    );
    let restored_parent: (Option<i64>, Option<String>) = cache
        .query_row(
            "SELECT parent.parent_id, parent.external_oid
             FROM commit_parents AS parent
             JOIN commits AS child ON child.commit_id = parent.commit_id
             WHERE child.oid = ?1 AND parent.position = 0",
            [&tip],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read restored parent link");
    let restored_id: i64 = cache
        .query_row(
            "SELECT commit_id FROM commits WHERE oid = ?1",
            [&restored],
            |row| row.get(0),
        )
        .expect("read restored commit ID");
    assert_eq!(restored_parent, (Some(restored_id), None));
    let ordered: Vec<(String, i64)> = cache
        .prepare(
            "SELECT oid, position FROM commits
             WHERE oid IN (?1, ?2, ?3) ORDER BY position",
        )
        .expect("prepare commit ordering query")
        .query_map([&first, &restored, &tip], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .expect("query restored history")
        .collect::<Result<_, _>>()
        .expect("read restored history");
    assert_eq!(
        ordered
            .iter()
            .map(|(oid, _)| oid.as_str())
            .collect::<Vec<_>>(),
        [first.as_str(), restored.as_str(), tip.as_str()],
        "index must restore every reachable commit in history order"
    );
    assert!(ordered.windows(2).all(|pair| pair[0].1 < pair[1].1));
}
