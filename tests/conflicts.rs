mod support;
use serde_json::Value;
use std::{fs, process::Output};
use support::{TestRepo, git, git_command, git_stdout};

fn commit(repo: &TestRepo, path: &str, text: &str, message: &str) -> String {
    let file_path = repo.dir.path().join(path);
    fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    fs::write(file_path, text).unwrap();
    git(repo.dir.path(), ["add", path]);
    git(repo.dir.path(), ["commit", "-m", message]);
    repo.head()
}

fn commit_symlink(repo: &TestRepo, path: &str, target: &str, message: &str) -> String {
    let target_path = repo.dir.path().join(".symlink-target");
    fs::write(&target_path, target).unwrap();
    let blob = git_stdout(repo.dir.path(), ["hash-object", "-w", ".symlink-target"]);
    fs::remove_file(target_path).unwrap();
    let cacheinfo = format!("120000,{blob},{path}");
    git(
        repo.dir.path(),
        ["update-index", "--add", "--cacheinfo", &cacheinfo],
    );
    fs::write(repo.dir.path().join(path), target).unwrap();
    git(repo.dir.path(), ["commit", "-m", message]);
    repo.head()
}
fn json(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn merge(repo: &TestRepo, branch: &str) {
    assert!(
        !git_command(repo.dir.path())
            .args(["merge", branch])
            .output()
            .unwrap()
            .status
            .success()
    );
}

fn reject_history_refresh_writes(repo: &TestRepo) {
    let cache = rusqlite::Connection::open(repo.cache_dir().join("cache.sqlite")).unwrap();
    cache
        .execute_batch("CREATE TRIGGER reject_history_refresh BEFORE INSERT ON hunks BEGIN SELECT RAISE(ABORT, 'injected conflict history refresh failure'); END;")
        .unwrap();
}
fn fixture() -> (TestRepo, String, String, String, String) {
    let repo = TestRepo::new();
    let base = commit(&repo, "a.txt", "base\n", "base");
    commit(&repo, "b.txt", "base\n", "base b");
    git(repo.dir.path(), ["branch", "other"]);
    let ours = commit(
        &repo,
        "a.txt",
        "ours\n",
        "change ours\n\nPreserve the local route.",
    );
    commit(&repo, "b.txt", "ours b\n", "change b");
    let unrelated = commit(&repo, "unrelated.txt", "ours\n", "unrelated distraction");
    repo.index();
    git(repo.dir.path(), ["checkout", "other"]);
    let theirs = commit(&repo, "a.txt", "theirs\n", "change theirs");
    commit(&repo, "b.txt", "theirs b\n", "change other b");
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "other");
    (repo, base, ours, theirs, unrelated)
}

#[test]
fn conflicts_rejects_missing_published_cache_without_initializing_it() {
    let (repo, _, _, _, _) = fixture();
    fs::remove_dir_all(repo.cache_dir()).unwrap();

    let output = repo.run(["conflicts", "--json"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("run `gitscry index` first"));
    assert!(!repo.cache_dir().exists());
}

#[test]
fn refreshes_both_conflict_sides_and_preserves_other_cached_history() {
    let repo = TestRepo::new();
    commit(&repo, "a.txt", "base\n", "base");
    git(repo.dir.path(), ["branch", "other"]);
    let unrelated = commit(
        &repo,
        "unrelated.txt",
        "cached\n",
        "UnrelatedCacheMarker published history",
    );
    repo.index();

    let ours = commit(&repo, "a.txt", "ours\n", "ours after initialization");
    git(repo.dir.path(), ["checkout", "other"]);
    let theirs = commit(&repo, "a.txt", "theirs\n", "theirs after initialization");
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "other");

    let before = state(&repo);
    let output = repo.run(["conflicts", "--json"]);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let report = json(output);
    assert!(stderr.contains("Refreshing pinned history"), "{stderr}");
    assert_eq!(report["coverage_complete"], true);
    let sides = report["files"][0]["sides"].as_array().unwrap();
    assert_eq!(sides[0]["leads"][0]["commit"], ours);
    assert_eq!(sides[1]["leads"][0]["commit"], theirs);
    assert_eq!(state(&repo), before);

    let retained = repo.run([
        "search",
        "UnrelatedCacheMarker",
        "--to-rev",
        &unrelated,
        "--json",
    ]);
    assert!(
        retained.status.success(),
        "{}",
        String::from_utf8_lossy(&retained.stderr)
    );
    assert!(String::from_utf8_lossy(&retained.stdout).contains("UnrelatedCacheMarker"));

    let again = repo.run(["conflicts", "--json"]);
    assert!(again.status.success());
    assert!(
        again.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&again.stdout).unwrap(),
        report
    );
}

#[test]
fn conflicts_reports_local_semantic_refresh_failure_but_keeps_history() {
    let (repo, _, _, _, _) = fixture();
    let cache = rusqlite::Connection::open(repo.cache_dir().join("cache.sqlite")).unwrap();
    cache
        .execute(
            "UPDATE metadata SET value = '1' WHERE key = 'semantic_enabled'",
            [],
        )
        .unwrap();
    drop(cache);

    let output = TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
        .args(["conflicts", "--json"])
        .env("GITSCRY_FULL_OUTPUT", "1")
        .output()
        .unwrap();
    let report = json(output);
    let warnings = report["warnings"].to_string();
    assert!(
        warnings.contains("automatic semantic refresh failed"),
        "{warnings}"
    );
    assert!(
        warnings.contains("pinned semantic model resource"),
        "{warnings}"
    );
    assert_eq!(report["files"].as_array().unwrap().len(), 2);
}

#[test]
fn conflicts_falls_back_to_cached_history_after_refresh_failure() {
    let repo = TestRepo::new();
    commit(
        &repo,
        "history.txt",
        "parent history\n",
        "unpublished parent history",
    );
    let shallow_boundary = commit(
        &repo,
        "history.txt",
        "boundary history\n",
        "shallow boundary",
    );
    commit(&repo, "a.txt", "base\n", "base");
    git(repo.dir.path(), ["branch", "other"]);
    let ours = commit(&repo, "a.txt", "ours\n", "ours");
    git(repo.dir.path(), ["checkout", "other"]);
    let theirs = commit(&repo, "a.txt", "theirs\n", "theirs");
    git(repo.dir.path(), ["checkout", "main"]);

    let tree = git_stdout(repo.dir.path(), ["write-tree"]);
    let cache_anchor = git_stdout(
        repo.dir.path(),
        [
            "commit-tree",
            &tree,
            "-p",
            &ours,
            "-p",
            &theirs,
            "-m",
            "cache both sides",
        ],
    );
    git(
        repo.dir.path(),
        ["update-ref", "refs/heads/cache-anchor", &cache_anchor],
    );
    git(repo.dir.path(), ["checkout", "cache-anchor"]);
    fs::write(
        repo.common_dir().join("shallow"),
        format!("{shallow_boundary}\n"),
    )
    .unwrap();
    repo.index();
    reject_history_refresh_writes(&repo);
    fs::remove_file(repo.common_dir().join("shallow")).unwrap();

    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "other");
    let report = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--json"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    );
    let warnings = report["warnings"].to_string();
    assert!(
        warnings.contains("automatic history refresh failed"),
        "{warnings}"
    );
    assert!(
        warnings.contains("injected conflict history refresh failure"),
        "{warnings}"
    );
    assert!(
        warnings.contains("incomplete history coverage"),
        "{warnings}"
    );
    assert_eq!(report["files"].as_array().unwrap().len(), 1);
}

#[test]
fn conflicts_rejects_failed_refresh_when_an_endpoint_is_not_cached() {
    let (repo, _, _, _, _) = fixture();
    let theirs = git_stdout(repo.dir.path(), ["rev-parse", "MERGE_HEAD"]);
    reject_history_refresh_writes(&repo);

    let output = repo.run(["conflicts", "--json"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(&theirs), "{stderr}");
    assert!(
        stderr.contains("outside the published cache generation"),
        "{stderr}"
    );
}

fn commit_files(repo: &TestRepo, files: &[(&str, &str)], message: &str) -> String {
    for (path, text) in files {
        let file_path = repo.dir.path().join(path);
        fs::create_dir_all(file_path.parent().unwrap()).unwrap();
        fs::write(file_path, text).unwrap();
        git(repo.dir.path(), ["add", path]);
    }
    git(repo.dir.path(), ["commit", "-m", message]);
    repo.head()
}

fn associated_fixture() -> TestRepo {
    let repo = TestRepo::new();
    commit_files(
        &repo,
        &[
            (
                "src/service.rs",
                "pub fn service_identity(name: &str) -> String {\n    normalize_identity(name)\n}\n",
            ),
            (
                "src/identity.rs",
                "pub fn identity_key(name: &str) -> String {\n    normalize_identity(name)\n}\n",
            ),
            (
                "src/caller.rs",
                "pub fn caller(name: &str) -> String {\n    normalize_identity(name)\n}\n",
            ),
            (
                "tests/identity_regression.rs",
                "fn identity_regression() {\n    assert_eq!(normalize_identity(\"before\"), \"before\");\n}\n",
            ),
            ("src/unrelated.rs", "fn unrelated_before() {}\n"),
        ],
        "base identity behavior",
    );
    git(repo.dir.path(), ["branch", "other"]);
    commit_files(
        &repo,
        &[
            (
                "src/service.rs",
                "pub fn service_identity(name: &str) -> String {\n    const LABEL: &str = \"secret\x1btag\";\n    normalize_identity(name, Owner::Service)\n}\n",
            ),
            (
                "src/identity.rs",
                "pub fn identity_key(name: &str) -> String {\n    normalize_identity(name, Owner::Identity)\n}\n",
            ),
            (
                "src/caller.rs",
                "pub fn caller(name: &str) -> String {\n    let label = \"secret\x1btag\";\n    let value = normalize_identity(name, Owner::Service);\n    log_identity(normalize_identity(name, Owner::Service));\n    audit_identity(normalize_identity(name, Owner::Service));\n    publish_identity(normalize_identity(name, Owner::Service));\n    export_identity(normalize_identity(name, Owner::Service));\n    value\n}\n",
            ),
            ("src/unrelated.rs", "fn unrelated_ours() {}\n"),
        ],
        "assign identity ownership and update callers",
    );
    commit(
        &repo,
        "docs/branch_only.md",
        "unrelated branch-only change\n",
        "unrelated branch commit",
    );
    repo.index();
    git(repo.dir.path(), ["checkout", "other"]);
    commit_files(
        &repo,
        &[
            (
                "src/service.rs",
                "pub fn service_identity(name: &str) -> String {\n    normalize_identity(name, IdentityKey::Canonical)\n}\n",
            ),
            (
                "src/identity.rs",
                "pub fn identity_key(name: &str) -> String {\n    normalize_identity(name, IdentityKey::Canonical)\n}\n",
            ),
            (
                "tests/identity_regression.rs",
                "fn identity_regression() {\n    assert_eq!(normalize_identity(\"ID-42\"), \"id-42\");\n}\n",
            ),
            ("docs/same_commit_noise.md", "unrelated same-commit note\n"),
        ],
        "fix identity handling and add regression coverage",
    );
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "other");
    repo
}

fn shared_history_fixture() -> (TestRepo, String, String, String, String, String) {
    let repo = TestRepo::new();
    commit(&repo, "a.txt", "route=old\n", "base");
    commit(&repo, "b.txt", "base b\n", "base b");
    let fix = commit(
        &repo,
        "a.txt",
        "route=fast\n",
        "fix: preserve the fast route",
    );
    git(repo.dir.path(), ["revert", "--no-edit", fix.as_str()]);
    let revert = repo.head();
    for version in 1..=4 {
        commit(
            &repo,
            "a.txt",
            &format!("route=intermediate-{version}\n"),
            &format!("routine path edit {version}"),
        );
    }
    let unrelated = commit(
        &repo,
        "unrelated.txt",
        "unrelated\n",
        "fix: unrelated historical behavior",
    );
    git(repo.dir.path(), ["branch", "other"]);
    let ours = commit(&repo, "a.txt", "route=ours\n", "change ours");
    repo.index();
    git(repo.dir.path(), ["checkout", "other"]);
    let theirs = commit(&repo, "a.txt", "route=theirs\n", "change theirs");
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "other");
    (repo, fix, revert, ours, theirs, unrelated)
}

#[test]
fn reports_reverted_shared_history_once_with_source_and_conflict_associations() {
    let (repo, fix, revert, ours, theirs, unrelated) = shared_history_fixture();
    let before = state(&repo);
    let report = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--json"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    );
    let file = &report["files"][0];
    let related = file["related_history"].as_array().unwrap();
    assert_eq!(related[0]["commit"], fix);
    assert_eq!(
        related.iter().filter(|lead| lead["commit"] == fix).count(),
        1
    );
    assert!(!related.iter().any(|lead| lead["commit"] == revert));
    let fix_lead = related.iter().find(|lead| lead["commit"] == fix).unwrap();
    assert_eq!(fix_lead["path"], "a.txt");
    assert_eq!(
        fix_lead["association"],
        "shared pre-merge-base path history"
    );
    assert_eq!(
        fix_lead["related_sides"],
        serde_json::json!(["ours", "theirs"])
    );
    assert_eq!(fix_lead["reverted_by"]["commit"], revert);
    assert_eq!(fix_lead["recorded_reason"], Value::Null);
    assert!(
        fix_lead["reason_source"]
            .as_str()
            .unwrap()
            .contains("revert")
    );
    assert!(
        fix_lead["selection_basis"]
            .as_array()
            .is_some_and(|basis| !basis.is_empty())
    );
    assert!(!fix_lead["regions"].as_array().unwrap().is_empty());
    assert_eq!(file["related_history_total"], 6);
    assert_eq!(file["related_history_truncated"], true);
    assert_eq!(file["sides"][0]["leads"][0]["commit"], ours);
    assert_eq!(file["sides"][1]["leads"][0]["commit"], theirs);
    assert!(!related.iter().any(|lead| lead["commit"] == unrelated));

    let text = repo.run(["conflicts"]);
    assert!(text.status.success());
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(text.contains(&fix) && text.contains(&revert));
    assert!(text.contains("shared pre-merge-base path history"));
    assert!(text.contains("none recorded"));
    assert_eq!(state(&repo), before);
}

#[test]
fn bounds_shared_history_per_conflicted_file() {
    let repo = TestRepo::new();
    commit(&repo, "a.txt", "value=0\n", "base a");
    commit(&repo, "b.txt", "base b\n", "base b");
    for value in 1..=5 {
        commit(
            &repo,
            "a.txt",
            &format!("value={value}\n"),
            &format!("shared change {value}"),
        );
    }
    git(repo.dir.path(), ["branch", "other"]);
    commit(&repo, "a.txt", "value=ours\n", "change ours");
    repo.index();
    git(repo.dir.path(), ["checkout", "other"]);
    commit(&repo, "a.txt", "value=theirs\n", "change theirs");
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "other");

    let report = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--json"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    );
    let file = &report["files"][0];
    assert_eq!(file["related_history"].as_array().unwrap().len(), 3);
    assert_eq!(file["related_history_total"], 6);
    assert_eq!(file["related_history_truncated"], true);
    assert_eq!(report["limits"]["related_history_per_file"], 3);
}
fn historical_conflict_fixture(
    historical_path: &str,
    custom_driver: bool,
) -> (TestRepo, String, String, String, String) {
    historical_conflict_fixture_with(historical_path, custom_driver, false, false)
}

fn historical_conflict_fixture_with(
    historical_path: &str,
    custom_driver: bool,
    one_sided: bool,
    symlink_conflict: bool,
) -> (TestRepo, String, String, String, String) {
    let repo = TestRepo::new();
    let mut base = commit(
        &repo,
        historical_path,
        "setting=base\nunique-context=anchor\n",
        "shared base",
    );
    if symlink_conflict {
        git(repo.dir.path(), ["config", "core.symlinks", "false"]);
        base = commit(&repo, "symlink.txt", "base-target\n", "base symlink path");
    }
    if custom_driver {
        fs::write(
            repo.dir.path().join(".gitattributes"),
            "unselected.txt merge=external-sentinel\n",
        )
        .unwrap();
        fs::write(repo.dir.path().join("unselected.txt"), "content\n").unwrap();
        git(repo.dir.path(), ["add", ".gitattributes", "unselected.txt"]);
        git(
            repo.dir.path(),
            ["commit", "-m", "add unrelated custom merge attribute"],
        );
        base = repo.head();
    }
    git(repo.dir.path(), ["branch", "historical-theirs"]);
    let mut historical_ours = commit(
        &repo,
        historical_path,
        "setting=ours-old\nunique-context=anchor\n",
        "historical ours",
    );
    if custom_driver {
        fs::write(repo.dir.path().join("unselected.txt"), "ours\n").unwrap();
        git(repo.dir.path(), ["add", "unselected.txt"]);
        git(repo.dir.path(), ["commit", "-m", "change unrelated ours"]);
        historical_ours = repo.head();
    }
    repo.index();
    git(repo.dir.path(), ["checkout", "historical-theirs"]);
    let mut historical_theirs = commit(
        &repo,
        historical_path,
        "setting=theirs-old\nunique-context=anchor\n",
        "historical theirs",
    );
    if custom_driver {
        fs::write(repo.dir.path().join("unselected.txt"), "theirs\n").unwrap();
        git(repo.dir.path(), ["add", "unselected.txt"]);
        git(repo.dir.path(), ["commit", "-m", "change unrelated theirs"]);
        historical_theirs = repo.head();
    }
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "historical-theirs");
    if custom_driver {
        fs::write(repo.dir.path().join("unselected.txt"), "resolved\n").unwrap();
        git(repo.dir.path(), ["add", "unselected.txt"]);
    }
    let historical_merge = commit(
        &repo,
        historical_path,
        "setting=ours-old\nunique-context=anchor\n",
        "historical resolution equals first parent",
    );
    repo.index();
    if custom_driver {
        git(repo.dir.path(), ["rm", ".gitattributes"]);
        git(
            repo.dir.path(),
            ["commit", "-m", "remove unrelated custom merge attribute"],
        );
    }

    if one_sided {
        git(repo.dir.path(), ["branch", "current-theirs", &base]);
    } else {
        git(repo.dir.path(), ["branch", "current-theirs"]);
    }
    if historical_path != "conflict.txt" {
        git(repo.dir.path(), ["mv", historical_path, "conflict.txt"]);
    }
    commit(
        &repo,
        "conflict.txt",
        "setting=ours-now\nunique-context=anchor\n",
        "current ours",
    );
    if symlink_conflict {
        commit_symlink(
            &repo,
            "symlink.txt",
            "ours-target\n",
            "current ours symlink",
        );
    }
    git(repo.dir.path(), ["checkout", "current-theirs"]);
    if historical_path != "conflict.txt" {
        git(repo.dir.path(), ["mv", historical_path, "conflict.txt"]);
    }
    commit(
        &repo,
        "conflict.txt",
        "setting=theirs-now\nunique-context=anchor\n",
        "current theirs",
    );
    if symlink_conflict {
        commit_symlink(
            &repo,
            "symlink.txt",
            "theirs-target\n",
            "current theirs symlink",
        );
    }
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "current-theirs");

    (
        repo,
        base,
        historical_ours,
        historical_theirs,
        historical_merge,
    )
}

#[test]
fn historical_metadata_preserves_multiple_paths_and_candidates() {
    let repo = TestRepo::new();
    let paths = ["a.txt", "literal[1].txt", "c.txt"];
    let write = |text: &str, message: &str| {
        commit_files(
            &repo,
            &paths.iter().map(|path| (*path, text)).collect::<Vec<_>>(),
            message,
        )
    };
    write("setting=base\nunique-context=anchor\n", "base");
    let mut merges = Vec::new();
    for cycle in 0..2 {
        let side = format!("historical-{cycle}");
        git(repo.dir.path(), ["branch", &side]);
        let ours = format!("setting=ours-{cycle}\nunique-context=anchor\n");
        write(&ours, "historical ours");
        git(repo.dir.path(), ["checkout", &side]);
        write(
            &format!("setting=theirs-{cycle}\nunique-context=anchor\n"),
            "historical theirs",
        );
        git(repo.dir.path(), ["checkout", "main"]);
        merge(&repo, &side);
        merges.push(write(&ours, "historical resolution"));
    }
    repo.index();
    git(repo.dir.path(), ["branch", "current-theirs"]);
    write("setting=ours-now\nunique-context=anchor\n", "current ours");
    git(repo.dir.path(), ["checkout", "current-theirs"]);
    write(
        "setting=theirs-now\nunique-context=anchor\n",
        "current theirs",
    );
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "current-theirs");
    let before = state_with_paths(&repo, &paths);
    let report = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--json"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    );
    let history = &report["historical_cases"];
    assert_eq!(history["status"], "complete", "{history}");
    let files = history["files"].as_array().unwrap();
    assert_eq!(history["candidates"]["discovered"], 6);
    assert_eq!(history["candidates"]["examined"], 6);
    assert_eq!(history["candidates"]["irrelevant"], 6);
    assert_eq!(history["candidates"]["unchecked"], 0);
    assert_eq!(files.len(), 3, "{history}");
    let mut ordering = None;
    for file in files {
        let path = file["path"].as_str().unwrap();
        assert!(paths.contains(&path), "{file}");
        assert_eq!(file["candidate_merges"], 2);
        assert_eq!(file["candidates_examined"], 2);
        assert_eq!(file["candidates_irrelevant"], 2);
        assert_eq!(file["candidates_unchecked"], 0);
        let cases = file["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 2, "{file}");
        let ids = cases
            .iter()
            .map(|case| case["merge_commit"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        for (cycle, oid) in merges.iter().enumerate() {
            let case = cases
                .iter()
                .find(|case| case["merge_commit"] == *oid)
                .unwrap();
            assert_eq!(case["result"]["path"], path);
            assert_eq!(
                case["result"]["excerpt"],
                format!("setting=ours-{cycle}\nunique-context=anchor\n")
            );
            assert_eq!(case["related_sides"], serde_json::json!(["ours", "theirs"]));
        }
        if let Some(expected) = &ordering {
            assert_eq!(&ids, expected);
        } else {
            ordering = Some(ids);
        }
    }
    let bounded = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--json", "--max-historical-checks", "4"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    );
    let history = &bounded["historical_cases"];
    assert_eq!(history["status"], "partial", "{history}");
    assert_eq!(history["candidates"]["discovered"], 6);
    assert_eq!(history["candidates"]["examined"], 4);
    assert_eq!(history["candidates"]["irrelevant"], 4);
    assert_eq!(history["candidates"]["unchecked"], 2);
    assert_eq!(history["candidates"]["limited"], true);
    assert_eq!(history["files_analyzed"], 3);
    assert_eq!(history["files_with_cases"], 2);
    let files = history["files"].as_array().unwrap();
    assert_eq!(files.len(), 2, "empty file stays hidden: {history}");
    for file in files {
        assert_eq!(file["candidate_merges"], 2);
        assert_eq!(file["candidates_examined"], 2);
        assert_eq!(file["candidates_irrelevant"], 2);
        assert_eq!(file["candidates_unchecked"], 0);
    }
    assert_eq!(state_with_paths(&repo, &paths), before);
}

#[test]
fn reports_a_shared_historical_conflict_even_when_its_result_matches_first_parent() {
    let (repo, base, historical_ours, historical_theirs, historical_merge) =
        historical_conflict_fixture("conflict.txt", false);
    let before = state_with_paths(&repo, &["conflict.txt"]);

    let report = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--json"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    );
    let history = &report["historical_cases"];
    assert_eq!(
        history["status"], "complete",
        "historical report: {history}"
    );
    assert!(history["git_version"].as_str().is_some());
    let cases = history["files"][0]["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 1, "historical report: {history}");
    let case = &cases[0];
    assert_eq!(case["merge_commit"], historical_merge);
    assert_eq!(case["merge_base"]["commit"], base);
    assert_eq!(case["parents"][0]["commit"], historical_ours);
    assert_eq!(case["parents"][1]["commit"], historical_theirs);
    assert_eq!(case["related_sides"], serde_json::json!(["ours", "theirs"]));
    assert_eq!(history["candidates"]["discovered"], 1);
    assert_eq!(history["candidates"]["examined"], 1);
    assert_eq!(history["candidates"]["nonconflicting"], 0);
    assert_eq!(history["candidates"]["irrelevant"], 1);
    assert_eq!(history["candidates"]["unchecked"], 0);
    assert_eq!(history["files"][0]["candidates_examined"], 1);
    assert_eq!(history["files"][0]["candidates_unchecked"], 0);
    assert_eq!(
        case["result"]["excerpt"],
        "setting=ours-old\nunique-context=anchor\n"
    );
    assert!(
        case["reconstructed_conflict"]["excerpt"]
            .as_str()
            .unwrap()
            .contains("<<<<<<<")
    );

    let human = repo.run(["conflicts"]);
    assert!(human.status.success());
    let human = String::from_utf8_lossy(&human.stdout);
    assert!(human.contains("Historical merge cases"));
    assert!(human.contains(&historical_merge));
    for evidence in [
        &case["merge_base"],
        &case["parents"][0],
        &case["parents"][1],
        &case["reconstructed_conflict"],
        &case["result"],
    ] {
        for field in ["path", "blob", "excerpt"] {
            let value = evidence[field].as_str().unwrap();
            let needle = if field == "excerpt" {
                value.lines().next().unwrap_or(value)
            } else {
                value
            };
            assert!(
                human.contains(needle),
                "human output omits {field}: {needle}"
            );
        }
    }
    assert_eq!(state_with_paths(&repo, &["conflict.txt"]), before);
}

#[test]
fn bounds_historical_checks_with_max_historical_checks() {
    let (repo, _, _, _, historical_merge) = historical_conflict_fixture("conflict.txt", false);

    // A budget of 1 admits the single discovered candidate.
    let report = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--json", "--max-historical-checks", "1"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    );
    let history = &report["historical_cases"];
    assert_eq!(
        history["status"], "complete",
        "historical report: {history}"
    );
    assert_eq!(history["candidates"]["discovered"], 1);
    assert_eq!(history["candidates"]["examined"], 1);
    assert_eq!(history["candidates"]["unchecked"], 0);
    assert_eq!(history["candidates"]["budget"], 1);
    assert_eq!(history["candidates"]["limited"], false);
    let cases = history["files"][0]["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 1, "historical report: {history}");
    assert_eq!(cases[0]["merge_commit"], historical_merge);

    // A zero budget keeps discovery complete but examines nothing.
    let report = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--json", "--max-historical-checks", "0"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    );
    let history = &report["historical_cases"];
    assert_eq!(history["status"], "partial", "historical report: {history}");
    assert_eq!(history["candidates"]["discovered"], 1);
    assert_eq!(history["candidates"]["examined"], 0);
    assert_eq!(history["candidates"]["unchecked"], 1);
    assert_eq!(history["candidates"]["budget"], 0);
    assert_eq!(history["candidates"]["limited"], true);
    assert!(
        history["files"].as_array().unwrap().is_empty(),
        "historical report: {history}"
    );
    assert!(
        history["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason.as_str().unwrap().contains("budget expired")),
        "historical report: {history}"
    );
    assert_eq!(history["files_analyzed"], 1);
    assert_eq!(history["files_with_cases"], 0);
    let human = repo.run(["conflicts", "--max-historical-checks", "0"]);
    assert!(human.status.success());
    let human = String::from_utf8_lossy(&human.stdout);
    assert!(human.contains("partial"));
}

#[test]
fn finds_a_historical_conflict_reachable_from_only_one_side() {
    let (repo, _, _, _, historical_merge) =
        historical_conflict_fixture_with("conflict.txt", false, true, false);
    let history = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--json"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    )["historical_cases"]
        .clone();
    assert_eq!(
        history["status"], "complete",
        "historical report: {history}"
    );
    let cases = history["files"][0]["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 1, "historical report: {history}");
    assert_eq!(cases[0]["merge_commit"], historical_merge);
    assert_eq!(cases[0]["related_sides"], serde_json::json!(["ours"]));
}

#[test]
fn non_regular_conflict_stages_do_not_hide_text_historical_cases() {
    let (repo, _, _, _, historical_merge) =
        historical_conflict_fixture_with("conflict.txt", false, false, true);
    let output = TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
        .args(["conflicts", "--json"])
        .env("GITSCRY_FULL_OUTPUT", "1")
        .output()
        .unwrap();
    let history = json(output)["historical_cases"].clone();
    assert_eq!(history["status"], "partial", "historical report: {history}");
    let text_file = history["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == "conflict.txt")
        .unwrap();
    assert_eq!(text_file["cases"][0]["merge_commit"], historical_merge);
}

#[test]
fn skips_a_clean_historical_merge_candidate() {
    let repo = TestRepo::new();
    let base = commit(
        &repo,
        "conflict.txt",
        "setting=base\nunique-context=anchor\n",
        "base",
    );
    git(repo.dir.path(), ["branch", "clean-side"]);
    commit(
        &repo,
        "conflict.txt",
        "setting=ours\nunique-context=anchor\n",
        "ours change",
    );
    git(repo.dir.path(), ["checkout", "clean-side"]);
    commit(&repo, "unrelated.txt", "safe\n", "clean side change");
    git(repo.dir.path(), ["checkout", "main"]);
    git(repo.dir.path(), ["merge", "clean-side"]);
    let clean_merge = repo.head();

    git(repo.dir.path(), ["branch", "current-theirs", &base]);
    git(repo.dir.path(), ["checkout", "current-theirs"]);
    commit(
        &repo,
        "conflict.txt",
        "setting=theirs\nunique-context=anchor\n",
        "theirs change",
    );
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "current-theirs");
    repo.index();

    let history = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--json"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    )["historical_cases"]
        .clone();
    assert_eq!(
        history["status"], "complete",
        "historical report: {history}"
    );
    assert_eq!(
        history["files_with_cases"], 0,
        "historical report: {history}"
    );
    assert!(history["files"].as_array().unwrap().is_empty());
    assert_eq!(
        git_stdout(
            repo.dir.path(),
            ["rev-list", "--parents", "-n", "1", &clean_merge],
        )
        .split_whitespace()
        .count(),
        3
    );
}

#[test]
fn maps_a_historical_conflict_through_a_later_file_rename() {
    let (repo, _, _, _, historical_merge) = historical_conflict_fixture("legacy.txt", false);
    let before = state_with_paths(&repo, &["conflict.txt", "legacy.txt"]);
    let report = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--json"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    );
    let history = &report["historical_cases"];
    assert_eq!(
        history["status"], "complete",
        "historical report: {history}"
    );
    let file = &history["files"][0];
    assert_eq!(file["path"], "conflict.txt");
    let case = &file["cases"][0];
    assert_eq!(case["merge_commit"], historical_merge);
    assert_eq!(case["merge_base"]["path"], "legacy.txt");
    assert_eq!(case["parents"][0]["path"], "legacy.txt");
    assert_eq!(case["parents"][1]["path"], "legacy.txt");
    assert_eq!(case["reconstructed_conflict"]["path"], "legacy.txt");
    assert_eq!(case["result"]["path"], "legacy.txt");
    assert_eq!(
        state_with_paths(&repo, &["conflict.txt", "legacy.txt"]),
        before
    );
}

#[test]
fn reports_association_basis_and_region_trace_for_historical_cases() {
    let (repo, _, _, _, historical_merge) = historical_conflict_fixture("conflict.txt", false);
    let before = state_with_paths(&repo, &["conflict.txt"]);
    let history = json(repo.run(["conflicts", "--json"]))["historical_cases"].clone();
    assert_eq!(
        history["status"], "complete",
        "historical report: {history}"
    );
    let case = &history["files"][0]["cases"][0];
    assert_eq!(case["merge_commit"], historical_merge);
    assert_eq!(
        case["association"],
        "same file incarnation and identical unique surrounding context"
    );
    let trace = &case["region_trace"];
    assert_eq!(trace["current_start_line"], 1);
    assert_eq!(trace["current_lines"], 5);
    assert_eq!(trace["historical_start_line"], 1);
    assert_eq!(trace["historical_lines"], 5);
    let human = repo.run(["conflicts"]);
    assert!(human.status.success());
    let human = String::from_utf8_lossy(&human.stdout);
    assert!(
        human.contains(
            "Association: same file incarnation and identical unique surrounding context"
        )
    );
    assert!(human.contains("Region trace: current lines 1-5, historical lines 1-5"));
    assert_eq!(state_with_paths(&repo, &["conflict.txt"]), before);
}

#[test]
fn falls_back_to_content_correspondence_without_unique_context() {
    let repo = TestRepo::new();
    // Historical merge: both sides replace the same line, no shared context
    // lines beyond the conflicting one, and no unique surrounding context in
    // the current file either -- only the conflicting line content itself is
    // unique on both sides.
    commit(&repo, "conflict.txt", "a\nb\nc\nshared=1\n", "base");
    git(repo.dir.path(), ["branch", "theirs"]);
    commit(&repo, "conflict.txt", "a\nb\nc\nours=1\n", "ours");
    git(repo.dir.path(), ["checkout", "theirs"]);
    commit(&repo, "conflict.txt", "a\nb\nc\ntheirs=1\n", "theirs");
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "theirs");
    let historical_merge = commit(&repo, "conflict.txt", "a\nb\nc\nours=1\n", "resolve ours");
    repo.index();

    // Current conflict: surrounding context differs from the historical file,
    // so context matching cannot be unique; content matching must be used.
    git(repo.dir.path(), ["branch", "current-theirs"]);
    commit(&repo, "conflict.txt", "x\ny\nz\nours=1\n", "current ours");
    git(repo.dir.path(), ["checkout", "current-theirs"]);
    commit(
        &repo,
        "conflict.txt",
        "x\ny\nz\ntheirs=1\n",
        "current theirs",
    );
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "current-theirs");
    repo.index();

    let before = state_with_paths(&repo, &["conflict.txt"]);
    let history = json(repo.run(["conflicts", "--json"]))["historical_cases"].clone();
    assert_eq!(
        history["status"], "complete",
        "historical report: {history}"
    );
    let case = &history["files"][0]["cases"][0];
    assert_eq!(case["merge_commit"], historical_merge);
    assert_eq!(
        case["association"],
        "same file incarnation and unique conflict-region content correspondence across historical edits"
    );
    let trace = &case["region_trace"];
    assert_eq!(trace["current_start_line"], 4);
    assert_eq!(trace["current_lines"], 5);
    assert_eq!(trace["historical_start_line"], 4);
    assert_eq!(trace["historical_lines"], 5);
    assert_eq!(state_with_paths(&repo, &["conflict.txt"]), before);
}

#[test]
fn skips_ambiguous_region_correspondence_with_recorded_reason() {
    let repo = TestRepo::new();
    // Two historical merge candidates for the same current conflict:
    //
    // 1. A merge whose replay leaves two conflict regions separated by a
    //    5-line gap (git needs at least 3 unchanged lines between changed
    //    stretches to keep hunks separate; a wide gap keeps both sides
    //    separate here). The current conflict recreates the same two-region
    //    shape with a matching but repeated surrounding context, so no region
    //    pair can be pinned down by a unique context or unique content window
    //    and the candidate must be skipped as ambiguous.
    // 2. A merge with a single-region conflict whose `ours` side is unique in
    //    the current file, which resolves to a case; without it the file
    //    would be reported only in the top-level reasons instead of as a
    //    file entry with candidates_skipped.
    commit(&repo, "conflict.txt", "a\nb\nc\nd\ne\nf\ng\n", "base");

    // First historical merge: two-region conflict, resolved to ours.
    git(repo.dir.path(), ["branch", "two-region-theirs"]);
    commit(&repo, "conflict.txt", "x1\nb\nc\nd\ne\nf\nx2\n", "ours");
    git(repo.dir.path(), ["checkout", "two-region-theirs"]);
    commit(&repo, "conflict.txt", "y1\nb\nc\nd\ne\nf\ny2\n", "theirs");
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "two-region-theirs");
    let two_region_merge = commit(
        &repo,
        "conflict.txt",
        "x1\nb\nc\nd\ne\nf\nx2\n",
        "resolve ours",
    );

    // Second historical merge: single-region conflict, resolved to ours.
    git(repo.dir.path(), ["branch", "single-region-theirs"]);
    commit(
        &repo,
        "conflict.txt",
        "x1\nb\nc\nd\ne\nf\nh-ours\n",
        "single ours",
    );
    git(repo.dir.path(), ["checkout", "single-region-theirs"]);
    commit(
        &repo,
        "conflict.txt",
        "x1\nb\nc\nd\ne\nf\nh-theirs\n",
        "single theirs",
    );
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "single-region-theirs");
    let single_region_merge = commit(
        &repo,
        "conflict.txt",
        "x1\nb\nc\nd\ne\nf\nh-ours\n",
        "single resolve ours",
    );
    repo.index();

    // Current conflict: two regions with repeated context on both sides.
    git(repo.dir.path(), ["branch", "current-theirs"]);
    commit(
        &repo,
        "conflict.txt",
        "now-ours=1\nb\nc\nd\ne\nf\nnow-ours=2\ng\n",
        "current ours",
    );
    git(repo.dir.path(), ["checkout", "current-theirs"]);
    commit(
        &repo,
        "conflict.txt",
        "now-theirs=1\nb\nc\nd\ne\nf\nnow-theirs=2\ng\n",
        "current theirs",
    );
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "current-theirs");
    repo.index();

    let before = state_with_paths(&repo, &["conflict.txt"]);
    let file = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--json"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    )["historical_cases"]["files"][0]
        .clone();
    // The ambiguous two-region candidate is retained (ranked below the unique
    // correspondence) with its limitation recorded on the case itself.
    assert_eq!(file["status"], "complete", "file report: {file}");
    assert_eq!(file["cases"].as_array().unwrap().len(), 2);
    assert_eq!(
        file["cases"][0]["merge_commit"], single_region_merge,
        "file report: {file}"
    );
    assert_eq!(file["cases"][1]["merge_commit"], two_region_merge);
    assert_eq!(
        file["cases"][1]["association"],
        "same file incarnation; no unique region correspondence"
    );
    assert!(file["cases"][1]["region"].is_null());
    assert!(
        file["cases"][1]["limitations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason
                .as_str()
                .unwrap()
                .contains("no precise region mapping")),
        "file report: {file}"
    );
    assert_eq!(state_with_paths(&repo, &["conflict.txt"]), before);
}

#[test]
fn ignores_user_global_merge_attributes_during_historical_replay() {
    let (repo, _, _, _, historical_merge) = historical_conflict_fixture("conflict.txt", false);
    let xdg_config = repo.user_data_dir().join("xdg-config");
    let attributes = xdg_config.join("git/attributes");
    fs::create_dir_all(attributes.parent().unwrap()).unwrap();
    fs::write(&attributes, "conflict.txt merge=union\n").unwrap();

    let output = TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
        .env("XDG_CONFIG_HOME", &xdg_config)
        .args(["conflicts", "--json"])
        .output()
        .expect("run gitscry with isolated user-global attributes");
    let history = json(output)["historical_cases"].clone();
    assert_eq!(
        history["status"], "complete",
        "historical report: {history}"
    );
    assert_eq!(
        history["files"][0]["cases"][0]["merge_commit"],
        historical_merge
    );
}

#[test]
fn skips_custom_attributes_without_running_an_external_driver() {
    let (repo, base, historical_ours, historical_theirs, _) =
        historical_conflict_fixture("conflict.txt", true);
    assert_eq!(
        git_stdout(
            repo.dir.path(),
            ["show", format!("{historical_ours}:unselected.txt").as_str()],
        ),
        "ours",
    );
    assert_eq!(
        git_stdout(
            repo.dir.path(),
            [
                "show",
                format!("{historical_theirs}:unselected.txt").as_str()
            ],
        ),
        "theirs",
    );
    let sentinel = repo.dir.path().join("external-driver-ran");
    let sentinel_path = sentinel.display().to_string().replace('\\', "/");
    let driver = format!("echo invoked > \"{}\"", sentinel_path.replace('"', "\\\""));
    git(
        repo.dir.path(),
        [
            "config",
            "--local",
            "merge.external-sentinel.driver",
            driver.as_str(),
        ],
    );
    assert_eq!(
        git_stdout(
            repo.dir.path(),
            [
                "check-attr",
                &format!("--source={base}"),
                "merge",
                "--",
                "unselected.txt",
            ],
        ),
        "unselected.txt: merge: external-sentinel",
    );
    assert_eq!(
        git_stdout(
            repo.dir.path(),
            [
                "config",
                "--local",
                "--get",
                "merge.external-sentinel.driver"
            ],
        ),
        driver
    );
    git(repo.dir.path(), ["merge", "--abort"]);
    git(
        repo.dir.path(),
        ["checkout", "--detach", historical_ours.as_str()],
    );
    let baseline = git_command(repo.dir.path())
        .args(["merge", "--no-edit", historical_theirs.as_str()])
        .output()
        .unwrap();
    assert!(!baseline.status.success());
    assert!(
        sentinel.exists(),
        "configured custom driver should be executable"
    );
    fs::remove_file(&sentinel).unwrap();
    git(repo.dir.path(), ["merge", "--abort"]);
    git(repo.dir.path(), ["checkout", "main"]);
    let current_merge = git_command(repo.dir.path())
        .args(["merge", "--no-edit", "current-theirs"])
        .output()
        .unwrap();
    assert!(!current_merge.status.success());

    let before = state_with_paths(&repo, &["conflict.txt", "unselected.txt", ".gitattributes"]);
    let object_count_before = git_stdout(repo.dir.path(), ["count-objects", "-v"]);
    let config_before = git_stdout(
        repo.dir.path(),
        [
            "config",
            "--local",
            "--get",
            "merge.external-sentinel.driver",
        ],
    );
    let report = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--json"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    );
    let history = &report["historical_cases"];
    assert_eq!(history["status"], "partial", "historical report: {history}");
    assert_eq!(history["files_analyzed"], 1);
    assert_eq!(history["files_with_cases"], 0);
    assert!(history["files"].as_array().unwrap().is_empty());
    assert!(
        history["reasons"][0]
            .as_str()
            .unwrap()
            .contains("external drivers are disabled")
    );
    assert!(
        !sentinel.exists(),
        "query invoked the external merge driver"
    );
    assert_eq!(
        state_with_paths(&repo, &["conflict.txt", "unselected.txt", ".gitattributes"]),
        before
    );
    assert_eq!(
        git_stdout(repo.dir.path(), ["count-objects", "-v"]),
        object_count_before
    );
    assert_eq!(
        git_stdout(
            repo.dir.path(),
            [
                "config",
                "--local",
                "--get",
                "merge.external-sentinel.driver"
            ],
        ),
        config_before
    );
}
fn state(repo: &TestRepo) -> Vec<Vec<u8>> {
    state_with_paths(repo, &["a.txt", "b.txt"])
}

fn state_with_paths(repo: &TestRepo, paths: &[&str]) -> Vec<Vec<u8>> {
    let root = repo.dir.path();
    let mut state = Vec::new();
    for path in paths {
        state.push(path.as_bytes().to_vec());
        match fs::read(root.join(path)) {
            Ok(contents) => {
                state.push(vec![1]);
                state.push(contents);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => state.push(vec![0]),
            Err(error) => panic!("reading working-tree path {path}: {error}"),
        }
    }
    for args in [
        vec!["ls-files", "--stage", "-z"],
        vec!["ls-files", "--unmerged", "-z"],
        vec!["show-ref", "--head"],
        vec!["status", "--porcelain=v1", "-z"],
        vec!["rev-parse", "HEAD"],
    ] {
        state.push(git_command(root).args(args).output().unwrap().stdout);
    }
    for name in ["MERGE_HEAD", "MERGE_MSG", "ORIG_HEAD", "index"] {
        state.push(fs::read(repo.common_dir().join(name)).unwrap());
    }
    state
}

#[test]
fn retrieves_both_sides_prepares_uncached_branch_and_reuses_history_without_git_changes() {
    let (repo, _base, ours, theirs, unrelated) = fixture();
    let before = state(&repo);
    let report = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--json"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    );
    assert_eq!(report["ours"], repo.head());
    assert_eq!(
        report["theirs"],
        git_stdout(repo.dir.path(), ["rev-parse", "other"])
    );
    assert_eq!(report["files"].as_array().unwrap().len(), 2);
    assert_eq!(report["coverage_complete"], true);
    let sides = report["files"][0]["sides"].as_array().unwrap();
    assert_eq!(sides[0]["name"], "ours");
    assert_eq!(sides[0]["leads"][0]["commit"], ours);
    assert_eq!(sides[1]["name"], "theirs");
    assert_eq!(sides[1]["leads"][0]["commit"], theirs);
    assert!(
        sides[0]["leads"][0]["recorded_reason"]
            .as_str()
            .unwrap()
            .contains("Preserve the local route")
    );
    assert!(sides[1]["leads"][0]["recorded_reason"].is_null());
    assert!(
        !sides[0]["leads"][0]["regions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(sides.iter().all(|side| {
        side["leads"]
            .as_array()
            .unwrap()
            .iter()
            .all(|lead| lead["commit"] != unrelated)
    }));
    let again = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--json"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    );
    assert_eq!(report, again);
    let text = repo.run(["conflicts"]);
    assert!(text.status.success());
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(text.contains(&ours) && text.contains(&theirs));
    assert!(text.contains("none recorded"));
    assert!(!text.contains("<<<<<<<"));
    assert_eq!(state(&repo), before);
    let selected = json(repo.run(["conflicts", "--path", "b.txt", "--json"]));
    assert_eq!(selected["files"].as_array().unwrap().len(), 1);
    assert_eq!(selected["files"][0]["path"], "b.txt");
    assert!(
        !repo
            .run(["conflicts", "--path", "missing"])
            .status
            .success()
    );
    assert!(!repo.run(["context"]).status.success());
}

#[test]
fn exposes_earlier_behavior_and_refactor_across_a_detected_rename() {
    let repo = TestRepo::new();
    commit(
        &repo,
        "lib.rs",
        "pub fn score(input: i32) -> i32 {\n    input\n}\n\nfn adjust(input: i32) -> i32 {\n    input\n}\n",
        "base score",
    );
    repo.index();
    git(repo.dir.path(), ["branch", "other"]);
    git(repo.dir.path(), ["checkout", "other"]);
    let earlier = commit(
        &repo,
        "lib.rs",
        "pub fn score(input: i32) -> i32 {\n    input\n}\n\nfn adjust(input: i32) -> i32 {\n    input + 1\n}\n",
        "fix score behavior\n\nPreserve the special score adjustment.",
    );
    git(repo.dir.path(), ["mv", "lib.rs", "score.rs"]);
    let refactor = commit(
        &repo,
        "score.rs",
        "pub fn score(input: i32) -> i32 {\n    adjust(input)\n}\n\nfn adjust(input: i32) -> i32 {\n    input + 1\n}\n",
        "refactor score calculation",
    );
    let unrelated = commit(
        &repo,
        "unrelated.txt",
        "unrelated\n",
        "unrelated distraction",
    );
    git(repo.dir.path(), ["checkout", "main"]);
    git(repo.dir.path(), ["mv", "lib.rs", "score.rs"]);
    commit(
        &repo,
        "score.rs",
        "pub fn score(input: i32) -> i32 {\n    input + 2\n}\n\nfn adjust(input: i32) -> i32 {\n    input\n}\n",
        "change score on ours",
    );
    merge(&repo, "other");

    let report = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--json"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    );
    let file = &report["files"][0];
    assert_eq!(file["path"], "score.rs");
    let sides = file["sides"].as_array().unwrap();
    let theirs = sides.iter().find(|side| side["name"] == "theirs").unwrap();
    let leads = theirs["leads"].as_array().unwrap();
    let commits = leads
        .iter()
        .map(|lead| lead["commit"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(commits.contains(&earlier.as_str()));
    assert!(commits.contains(&refactor.as_str()));
    assert!(!commits.contains(&unrelated.as_str()));

    let earlier_lead = leads.iter().find(|lead| lead["commit"] == earlier).unwrap();
    assert_eq!(earlier_lead["selection_basis"], "detected rename lineage");
    assert_eq!(earlier_lead["conflict_path"], "score.rs");
    assert_eq!(earlier_lead["path_changes"][0]["old_path"], "lib.rs");
    assert_eq!(earlier_lead["regions"][0]["new_path"], "lib.rs");

    let text = String::from_utf8_lossy(&repo.run(["conflicts"]).stdout).to_string();
    assert!(text.contains("Selection basis: detected rename lineage"));
    assert!(text.contains("Historical path: lib.rs"));
}

#[test]
fn does_not_join_a_reintroduced_path_to_an_older_incarnation() {
    let repo = TestRepo::new();
    commit(
        &repo,
        "lib.rs",
        "pub fn score() -> i32 { 0 }\n",
        "base score",
    );
    repo.index();
    git(repo.dir.path(), ["branch", "other"]);
    git(repo.dir.path(), ["checkout", "other"]);
    let earlier_incarnation = commit(
        &repo,
        "lib.rs",
        "pub fn score() -> i32 { 1 }\n",
        "fix original score behavior\n\nKeep the original scoring rule.",
    );
    git(repo.dir.path(), ["rm", "lib.rs"]);
    git(
        repo.dir.path(),
        ["commit", "-m", "remove original score file"],
    );
    let reintroduced = commit(
        &repo,
        "lib.rs",
        "pub fn score() -> i32 { 100 }\n",
        "introduce new score file",
    );
    let current = commit(
        &repo,
        "lib.rs",
        "pub fn score() -> i32 { 101 }\n",
        "adjust new score behavior",
    );
    git(repo.dir.path(), ["checkout", "main"]);
    commit(
        &repo,
        "lib.rs",
        "pub fn score() -> i32 { 2 }\n",
        "change score on ours",
    );
    merge(&repo, "other");

    let report = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--json"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    );
    assert_eq!(report["coverage_complete"], true);
    let theirs = report["files"][0]["sides"]
        .as_array()
        .unwrap()
        .iter()
        .find(|side| side["name"] == "theirs")
        .unwrap();
    let commits = theirs["leads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|lead| lead["commit"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(commits.contains(&reintroduced.as_str()));
    assert!(commits.contains(&current.as_str()));
    assert!(!commits.contains(&earlier_incarnation.as_str()));
}

#[test]
fn rename_does_not_claim_same_commit_reintroduction() {
    let repo = TestRepo::new();
    let original = "pub fn score() -> i32 {\n    0\n}\n\n// Original scoring implementation.\n";
    commit(&repo, "lib.rs", original, "base score");
    git(repo.dir.path(), ["mv", "lib.rs", "score.rs"]);
    commit(&repo, "score.rs", original, "rename lib.rs to score.rs");
    repo.index();
    git(repo.dir.path(), ["branch", "other"]);
    git(repo.dir.path(), ["checkout", "other"]);
    fs::write(
        repo.dir.path().join("score.rs"),
        "pub fn score() -> i32 {\n    1\n}\n\n// Original scoring implementation.\n",
    )
    .unwrap();
    fs::write(
        repo.dir.path().join("lib.rs"),
        "pub fn unrelated() -> i32 { 100 }\n",
    )
    .unwrap();
    git(repo.dir.path(), ["add", "--all"]);
    git(
        repo.dir.path(),
        ["commit", "-m", "change score and reintroduce lib.rs"],
    );
    let reintroduced = repo.head();

    git(repo.dir.path(), ["checkout", "main"]);
    commit(
        &repo,
        "score.rs",
        "pub fn score() -> i32 {\n    2\n}\n\n// Original scoring implementation.\n",
        "change score on ours",
    );
    merge(&repo, "other");

    let report = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--json"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    );
    assert_eq!(report["coverage_complete"], true);
    let file = report["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == "score.rs")
        .unwrap();
    let theirs = file["sides"]
        .as_array()
        .unwrap()
        .iter()
        .find(|side| side["name"] == "theirs")
        .unwrap();
    let lead = theirs["leads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|lead| lead["commit"] == reintroduced)
        .unwrap();
    let path_changes = lead["path_changes"].as_array().unwrap();
    assert_eq!(path_changes.len(), 1);
    assert_eq!(path_changes[0]["status"], "M");
    assert_eq!(path_changes[0]["old_path"], "score.rs");
    assert_eq!(path_changes[0]["new_path"], "score.rs");
}

#[test]
fn reports_same_commit_callers_tests_and_bounded_matching_hunks() {
    let repo = associated_fixture();
    let report = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--json"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    );
    let materials = report["associated_materials"].as_array().unwrap();
    assert_eq!(materials.len(), 2);
    assert_eq!(report["schema_version"], 7);
    assert_eq!(report["associated_materials_truncated"], false);

    let caller = materials
        .iter()
        .find(|material| material["path"] == "src/caller.rs")
        .unwrap();
    assert_eq!(caller["kind"], "changed_code");
    let caller_identities = caller["shared_identities"].as_array().unwrap();
    assert!(caller_identities.contains(&serde_json::json!("normalize_identity")));
    assert!(
        caller_identities
            .iter()
            .any(|identity| identity.as_str().unwrap().contains('\x1b'))
    );
    assert_eq!(caller["associated_with"].as_array().unwrap().len(), 2);
    assert!(
        caller["associated_with"]
            .as_array()
            .unwrap()
            .iter()
            .all(|association| association["side"] == "ours")
    );
    assert_eq!(caller["hunks"][0]["lines"].as_array().unwrap().len(), 4);
    assert_eq!(caller["hunks"][0]["excerpt_truncated"], true);
    assert_eq!(caller["hunks_truncated"], true);

    let test = materials
        .iter()
        .find(|material| material["path"] == "tests/identity_regression.rs")
        .unwrap();
    assert_eq!(test["kind"], "test");
    assert_eq!(
        test["shared_identities"],
        serde_json::json!(["normalize_identity"])
    );
    assert_eq!(test["associated_with"].as_array().unwrap().len(), 2);
    assert!(
        test["associated_with"]
            .as_array()
            .unwrap()
            .iter()
            .all(|association| association["side"] == "theirs")
    );
    assert!(materials.iter().all(|material| {
        !material["path"].as_str().unwrap().contains("unrelated")
            && material["hunks"].as_array().unwrap().len() <= 3
    }));
    for material in materials {
        let associations = material["associated_with"].as_array().unwrap();
        let mut conflict_paths = associations
            .iter()
            .map(|association| association["conflict_path"].as_str().unwrap())
            .collect::<Vec<_>>();
        conflict_paths.sort_unstable();
        assert_eq!(conflict_paths, ["src/identity.rs", "src/service.rs"]);
        for association in associations {
            assert_eq!(association["lead_commit"], material["commit"]);
            let file = report["files"]
                .as_array()
                .unwrap()
                .iter()
                .find(|file| file["path"] == association["conflict_path"])
                .unwrap();
            let side = file["sides"]
                .as_array()
                .unwrap()
                .iter()
                .find(|side| side["name"] == association["side"])
                .unwrap();
            let lead = side["leads"]
                .as_array()
                .unwrap()
                .iter()
                .find(|lead| lead["commit"] == association["lead_commit"])
                .unwrap();
            assert!(
                lead["associated_material_ids"]
                    .as_array()
                    .unwrap()
                    .contains(&material["id"])
            );
            assert_eq!(lead["associated_materials_truncated"], false);
        }
    }

    let selected = json(repo.run(["conflicts", "--path", "src/service.rs", "--json"]));
    assert_eq!(selected["files"].as_array().unwrap().len(), 1);
    assert_eq!(selected["files"][0]["path"], "src/service.rs");
    assert!(
        selected["associated_materials"]
            .as_array()
            .unwrap()
            .iter()
            .all(
                |material| material["associated_with"].as_array().unwrap().len() == 1
                    && material["associated_with"][0]["conflict_path"] == "src/service.rs"
            )
    );

    let text = repo.run(["conflicts"]);
    assert!(text.status.success());
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(text.contains("Associated material"));
    assert!(text.contains("src/caller.rs"));
    assert!(text.contains("tests/identity_regression.rs"));
    assert!(text.contains("normalize_identity"));
    assert!(!text.contains('\x1b'));
    assert!(text.contains("\\u{1b}"));
    assert!(text.contains("excerpt truncated"));
}

#[test]
fn reports_output_bounds_and_absent_reasons() {
    let (repo, _, _, _, _) = fixture();
    git(repo.dir.path(), ["merge", "--abort"]);
    git(repo.dir.path(), ["checkout", "other"]);
    commit(
        &repo,
        "a.txt",
        "more theirs\n",
        &format!("more\n\n{}", "r".repeat(1100)),
    );
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "other");
    let report = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--limit", "1", "--json"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    );
    let side = &report["files"][0]["sides"][1];
    assert_eq!(side["total_leads"], 2);
    assert_eq!(side["truncated"], true);
    assert_eq!(side["leads"][0]["message_truncated"], true);
    assert_eq!(
        side["leads"][0]["recorded_reason"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        1000
    );
    assert!(!repo.run(["conflicts", "--limit", "0"]).status.success());
}

#[test]
fn rejects_non_merge_operations_and_unrelated_histories() {
    let repo = TestRepo::new();
    commit(&repo, "a.txt", "base\n", "base");
    assert!(!repo.run(["conflicts"]).status.success());
    for name in ["rebase-merge", "rebase-apply"] {
        fs::create_dir(repo.common_dir().join(name)).unwrap();
        let output = repo.run(["conflicts"]);
        assert!(String::from_utf8_lossy(&output.stderr).contains("ordinary merge"));
        fs::remove_dir(repo.common_dir().join(name)).unwrap();
    }
    fs::write(repo.common_dir().join("CHERRY_PICK_HEAD"), repo.head()).unwrap();
    assert!(String::from_utf8_lossy(&repo.run(["conflicts"]).stderr).contains("cherry-pick"));
    fs::remove_file(repo.common_dir().join("CHERRY_PICK_HEAD")).unwrap();
    git(repo.dir.path(), ["checkout", "--orphan", "unrelated"]);
    commit(&repo, "a.txt", "unrelated\n", "root");
    git(repo.dir.path(), ["checkout", "main"]);
    assert!(
        !git_command(repo.dir.path())
            .args(["merge", "--allow-unrelated-histories", "unrelated"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let output = repo.run(["conflicts"]);
    assert!(String::from_utf8_lossy(&output.stderr).contains("one merge base"));
}

#[test]
fn explicitly_reports_file_level_and_binary_conflicts() {
    let repo = TestRepo::new();
    commit(&repo, "a.txt", "base\n", "base");
    commit(&repo, "binary", "base\0\n", "binary");
    repo.index();
    git(repo.dir.path(), ["branch", "other"]);
    git(repo.dir.path(), ["rm", "a.txt"]);
    git(repo.dir.path(), ["commit", "-m", "delete"]);
    let deletion = repo.head();
    commit(&repo, "binary", "ours\0\n", "binary ours");
    git(repo.dir.path(), ["checkout", "other"]);
    let modification = commit(&repo, "a.txt", "theirs\n", "edit");
    commit(&repo, "binary", "theirs\0\n", "binary theirs");
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "other");
    let report = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--json"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    );
    assert_eq!(report["coverage_complete"], false);
    let file = &report["files"][0];
    assert_eq!(file["file_level"], true);
    assert_eq!(file["index_stages"], serde_json::json!([1, 3]));
    assert!(file["unsupported"].is_null());
    let sides = file["sides"].as_array().unwrap();
    assert_eq!(sides.len(), 2);
    assert_eq!(sides[0]["name"], "ours");
    assert_eq!(sides[0]["available_paths"], serde_json::json!([]));
    assert_eq!(sides[1]["name"], "theirs");
    assert_eq!(sides[1]["available_paths"], serde_json::json!(["a.txt"]));
    assert!(
        sides[0]["leads"]
            .as_array()
            .unwrap()
            .iter()
            .any(|lead| lead["commit"] == deletion)
    );
    assert!(
        sides[1]["leads"]
            .as_array()
            .unwrap()
            .iter()
            .any(|lead| lead["commit"] == modification)
    );
    assert!(
        report["files"][1]["unsupported"]
            .as_str()
            .unwrap()
            .contains("binary")
    );
    assert!(report["files"][1]["sides"].as_array().unwrap().is_empty());
}

#[test]
fn traces_rename_delete_conflicts_alongside_text_conflicts_without_git_changes() {
    let repo = TestRepo::new();
    commit(
        &repo,
        "src/old.rs",
        "pub fn score(value: i32) -> i32 {\n    value\n}\n",
        "base score implementation",
    );
    commit(&repo, "notes.txt", "value=base\n", "base notes");
    repo.index();
    git(repo.dir.path(), ["branch", "other"]);

    git(repo.dir.path(), ["checkout", "other"]);
    let historical = commit(
        &repo,
        "src/old.rs",
        "pub fn score(value: i32) -> i32 {\n    value + 1\n}\n",
        "increment score",
    );
    git(repo.dir.path(), ["mv", "src/old.rs", "src/new.rs"]);
    git(repo.dir.path(), ["commit", "-m", "rename score file"]);
    commit(&repo, "notes.txt", "value=theirs\n", "edit notes on theirs");

    git(repo.dir.path(), ["checkout", "main"]);
    git(repo.dir.path(), ["rm", "--", "src/old.rs"]);
    git(repo.dir.path(), ["commit", "-m", "delete score file"]);
    let deletion = repo.head();
    commit(&repo, "notes.txt", "value=ours\n", "edit notes on ours");
    merge(&repo, "other");

    let paths = ["src/old.rs", "src/new.rs", "notes.txt"];
    let before = state_with_paths(&repo, &paths);
    let report = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--json"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    );
    assert_eq!(report["coverage_complete"], true);
    assert_eq!(report["files"].as_array().unwrap().len(), 2);

    let files = report["files"].as_array().unwrap();
    let renamed = files
        .iter()
        .find(|file| file["path"] == "src/new.rs")
        .unwrap();
    assert_eq!(renamed["file_level"], true);
    assert_eq!(renamed["index_stages"], serde_json::json!([1, 3]));
    assert!(renamed["unsupported"].is_null());
    let sides = renamed["sides"].as_array().unwrap();
    assert_eq!(sides.len(), 2);
    assert_eq!(sides[0]["name"], "ours");
    assert_eq!(sides[0]["index_stage_present"], false);
    assert_eq!(sides[0]["available_paths"], serde_json::json!([]));
    assert_eq!(sides[1]["name"], "theirs");
    assert_eq!(sides[1]["index_stage_present"], true);
    assert_eq!(
        sides[1]["available_paths"],
        serde_json::json!(["src/new.rs"])
    );
    let deletion_lead = sides[0]["leads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|lead| lead["commit"] == deletion)
        .unwrap();
    assert_eq!(deletion_lead["selection_basis"], "detected rename lineage");
    assert_eq!(deletion_lead["path_changes"][0]["old_path"], "src/old.rs");
    assert!(deletion_lead["path_changes"][0]["new_path"].is_null());
    assert!(
        sides[1]["leads"]
            .as_array()
            .unwrap()
            .iter()
            .any(|lead| lead["commit"] == historical)
    );

    let text = files
        .iter()
        .find(|file| file["path"] == "notes.txt")
        .unwrap();
    assert_eq!(text["file_level"], false);
    assert_eq!(text["index_stages"], serde_json::json!([1, 2, 3]));
    assert_eq!(text["sides"].as_array().unwrap().len(), 2);

    let filtered = json(repo.run(["conflicts", "--path", "src/new.rs", "--json"]));
    assert_eq!(filtered["files"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["files"][0]["path"], "src/new.rs");
    let rendered = repo.run(["conflicts", "--path", "src/new.rs"]);
    assert!(rendered.status.success());
    let rendered = String::from_utf8_lossy(&rendered.stdout);
    assert!(rendered.contains("File-level conflict"));
    assert!(rendered.contains("Index stages: 1, 3"));
    assert!(rendered.contains(&deletion));
    assert!(rendered.contains("Historical path: src/old.rs"));
    assert_eq!(state_with_paths(&repo, &paths), before);
}

#[test]
fn rejects_multiple_endpoints_and_reports_cache_preparation_failure() {
    let (repo, _, _, _, _) = fixture();
    let merge_head = repo.common_dir().join("MERGE_HEAD");
    let original = fs::read_to_string(&merge_head).unwrap();
    fs::write(&merge_head, format!("{original}{original}")).unwrap();
    let output = repo.run(["conflicts"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("multiple merge endpoints"));
    fs::write(&merge_head, original).unwrap();
    fs::remove_dir_all(repo.cache_dir()).unwrap();
    fs::write(repo.cache_dir(), "obstruct cache preparation").unwrap();
    let output = repo.run(["conflicts", "--json"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cache"));
}

#[test]
fn shallow_history_after_initialization_reports_incomplete_coverage() {
    let (repo, _, ours, theirs, _) = fixture();
    let base = git_stdout(repo.dir.path(), ["merge-base", "main", "other"]);
    fs::write(repo.common_dir().join("shallow"), format!("{base}\n")).unwrap();
    let report = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--json"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    );
    assert_eq!(report["coverage_complete"], false);
    assert!(!report["warnings"].as_array().unwrap().is_empty());
    assert_eq!(report["files"][0]["sides"][0]["leads"][0]["commit"], ours);
    assert_eq!(report["files"][0]["sides"][1]["leads"][0]["commit"], theirs);
    let again = TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
        .args(["conflicts", "--json"])
        .env("GITSCRY_FULL_OUTPUT", "1")
        .output()
        .unwrap();
    assert!(again.status.success());
    assert!(
        again.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
    assert_eq!(json(again), report);
}

#[test]
fn discloses_non_utf8_commit_message_decoding() {
    let (repo, _, _, _, _) = fixture();
    git(repo.dir.path(), ["merge", "--abort"]);
    git(repo.dir.path(), ["checkout", "other"]);
    fs::write(repo.dir.path().join("a.txt"), "latest theirs\n").unwrap();
    git(repo.dir.path(), ["add", "a.txt"]);
    fs::write(repo.dir.path().join("message"), b"latest\n\nReason: \xff\n").unwrap();
    git(
        repo.dir.path(),
        [
            "-c",
            "i18n.commitEncoding=ISO-8859-1",
            "commit",
            "-F",
            "message",
        ],
    );
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "other");
    let report = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--json"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    );
    assert_eq!(
        report["files"][0]["sides"][1]["leads"][0]["message_lossy"],
        true
    );
    assert!(
        report["limitations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str().unwrap().contains("UTF-8"))
    );
}

#[test]
fn keeps_a_historical_case_whose_committed_result_is_not_inspectable_text() {
    let repo = TestRepo::new();
    let base = commit(
        &repo,
        "conflict.txt",
        "setting=base
unique-context=anchor
",
        "base",
    );
    git(repo.dir.path(), ["branch", "historical-theirs"]);
    let historical_ours = commit(
        &repo,
        "conflict.txt",
        "setting=ours-old
unique-context=anchor
",
        "historical ours",
    );
    git(repo.dir.path(), ["checkout", "historical-theirs"]);
    let historical_theirs = commit(
        &repo,
        "conflict.txt",
        "setting=theirs-old
unique-context=anchor
",
        "historical theirs",
    );
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "historical-theirs");
    // Resolve the historical conflict by committing binary content: the case
    // stays reportable, but its committed result has no text excerpt.
    fs::write(
        repo.dir.path().join("conflict.txt"),
        [0u8, 0xff, 0x80, b'b', b'i', b'n', b'a', b'r', b'y'],
    )
    .unwrap();
    git(repo.dir.path(), ["add", "conflict.txt"]);
    git(
        repo.dir.path(),
        ["commit", "-m", "historical resolution is binary"],
    );
    let historical_merge = repo.head();

    git(repo.dir.path(), ["branch", "current-theirs", &base]);
    git(repo.dir.path(), ["checkout", "current-theirs"]);
    commit(
        &repo,
        "conflict.txt",
        "setting=theirs-now
unique-context=anchor
",
        "current theirs",
    );
    git(repo.dir.path(), ["checkout", "main"]);
    commit(
        &repo,
        "conflict.txt",
        "setting=ours-now
unique-context=anchor
",
        "current ours",
    );
    merge(&repo, "current-theirs");
    repo.index();

    let report = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--json"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    );
    let history = &report["historical_cases"];
    assert_eq!(
        history["status"], "complete",
        "historical report: {history}"
    );
    let case = &history["files"][0]["cases"][0];
    assert_eq!(case["merge_commit"], historical_merge);
    assert!(case["result"].is_null(), "historical report: {case}");
    assert_eq!(case["result_status"], "unreadable");
    let limitations = case["limitations"].as_array().unwrap();
    assert!(
        limitations
            .iter()
            .any(|value| value.as_str().unwrap().contains("not bounded UTF-8 text")),
        "historical report: {history}"
    );
    assert_eq!(case["parents"][0]["commit"], historical_ours);
    assert_eq!(case["parents"][1]["commit"], historical_theirs);
    assert!(case["merge_base"]["excerpt"].is_string());

    let human = repo.run(["conflicts"]);
    assert!(human.status.success());
    let human = String::from_utf8_lossy(&human.stdout);
    assert!(human.contains("not available (unreadable)"), "{human}");
}

#[test]
fn ranks_historical_cases_by_region_strength_then_recency() {
    let repo = TestRepo::new();
    let base = commit(
        &repo,
        "conflict.txt",
        "setting=base
shared-anchor=1
",
        "base",
    );
    git(repo.dir.path(), ["branch", "side-a"]);
    let _weak_ours = commit(
        &repo,
        "conflict.txt",
        "setting=weak-ours
shared-anchor=1
",
        "weak historical ours",
    );
    git(repo.dir.path(), ["checkout", "side-a"]);
    let _weak_theirs = commit(
        &repo,
        "conflict.txt",
        "setting=weak-theirs
shared-anchor=1
",
        "weak historical theirs",
    );
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "side-a");
    commit(
        &repo,
        "conflict.txt",
        "setting=weak-ours
shared-anchor=1
",
        "weak historical resolution",
    );
    let weak_merge = repo.head();

    // A later merge whose conflict region shares no surrounding context with
    // the current conflict, and whose merge commit is more recent.
    commit(
        &repo,
        "unrelated.txt",
        "a
",
        "advance main clock",
    );
    git(repo.dir.path(), ["branch", "side-b"]);
    commit(
        &repo,
        "conflict.txt",
        "setting=strong-ours
shared-anchor=2
",
        "strong historical ours",
    );
    git(repo.dir.path(), ["checkout", "side-b"]);
    commit(
        &repo,
        "conflict.txt",
        "setting=strong-theirs
shared-anchor=2
",
        "strong historical theirs",
    );
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "side-b");
    commit(
        &repo,
        "conflict.txt",
        "setting=strong-ours
shared-anchor=2
",
        "strong historical resolution",
    );
    let strong_merge = repo.head();

    // Current conflict: both sides share the historical strong anchor as
    // unique surrounding context; the weak anchor appears twice in the file.
    git(repo.dir.path(), ["branch", "current-theirs", &base]);
    git(repo.dir.path(), ["checkout", "current-theirs"]);
    commit(
        &repo,
        "conflict.txt",
        "setting=theirs-now
shared-anchor=2
",
        "current theirs",
    );
    git(repo.dir.path(), ["checkout", "main"]);
    commit(
        &repo,
        "conflict.txt",
        "setting=ours-now
shared-anchor=2
",
        "current ours",
    );
    merge(&repo, "current-theirs");
    repo.index();

    let report = json(
        TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
            .args(["conflicts", "--json"])
            .env("GITSCRY_FULL_OUTPUT", "1")
            .output()
            .unwrap(),
    );
    let history = &report["historical_cases"];
    assert_eq!(
        history["status"], "complete",
        "historical report: {history}"
    );
    let cases = history["files"][0]["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 2, "historical report: {history}");
    assert_eq!(cases[0]["merge_commit"], strong_merge);
    assert_eq!(cases[1]["merge_commit"], weak_merge);
    assert_eq!(
        cases[0]["region"]["after"],
        serde_json::json!(["shared-anchor=2"])
    );
    assert!(cases[1]["region"].is_null());
    assert!(!cases[1]["limitations"].as_array().unwrap().is_empty());
    // Human output parity: the ranked order and limitation prose match.
    let human = repo.run(["conflicts"]);
    assert!(human.status.success());
    let human = String::from_utf8_lossy(&human.stdout);
    let strong_pos = human.find(&strong_merge).unwrap();
    let weak_pos = human.find(&weak_merge).unwrap();
    assert!(strong_pos < weak_pos);
    assert!(human.contains("Limitation:"));
}
