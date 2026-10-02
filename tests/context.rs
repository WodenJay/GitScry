mod support;

use serde_json::Value;
use std::{fs, process::Output};
use support::{TestRepo, git};

fn commit(repo: &TestRepo, files: &[(&str, &str)], message: &str) {
    for (path, content) in files {
        let path = repo.dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    git(repo.dir.path(), ["add", "--all"]);
    git(repo.dir.path(), ["commit", "-m", message]);
}

fn json(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn net_cancellation_succeeds_without_cache() {
    let repo = TestRepo::new();
    commit(&repo, &[("src/a.rs", "base\n")], "base");
    fs::write(repo.dir.path().join("src/a.rs"), "staged\n").unwrap();
    git(repo.dir.path(), ["add", "src/a.rs"]);
    fs::write(repo.dir.path().join("src/a.rs"), "base\n").unwrap();
    let report = json(repo.run(["context", "--json"]));
    assert_eq!(report["kind"], "context");
    assert_eq!(report["input"]["changes"], serde_json::json!([]));
    assert_eq!(report["suggestions"], serde_json::json!([]));
    assert!(report["cache_tip"].is_null());
    assert!(!repo.dir.path().join(".gitscry").exists());
    let staged = repo.run(["context", "--staged"]);
    assert!(!staged.status.success());
    assert!(String::from_utf8_lossy(&staged.stderr).contains("gitscry index"));
}

#[test]
fn clean_input_preserves_time_scope_validation_without_opening_cache() {
    let repo = TestRepo::new();
    for args in [
        vec!["context", "--since", "not-a-date"],
        vec!["context", "--until", "not-a-date"],
        vec!["context", "--since", "2002-01-01", "--until", "2001-01-01"],
    ] {
        let output = repo.run(args);
        assert!(!output.status.success());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("gitscry index"));
    }
    let report = json(repo.run([
        "context",
        "--since",
        "2000-01-01",
        "--until",
        "2001-01-01",
        "--json",
    ]));
    assert_eq!(report["input"]["changes"], serde_json::json!([]));
    assert!(!repo.dir.path().join(".gitscry").exists());
}

#[test]
fn returns_cited_paths_once_and_excludes_selected_paths() {
    let repo = TestRepo::new();
    commit(
        &repo,
        &[
            ("src/a.rs", "base\n"),
            ("src/b.rs", "base\n"),
            ("tests/a.rs", "base\n"),
        ],
        "base",
    );
    commit(
        &repo,
        &[
            ("src/a.rs", "next\n"),
            ("src/b.rs", "next\n"),
            ("tests/a.rs", "next\n"),
        ],
        "together",
    );
    let tip = repo.head();
    repo.index();
    fs::write(repo.dir.path().join("src/a.rs"), "current\n").unwrap();
    let report = json(TestRepo::run_at(
        &repo.dir.path().join("src"),
        ["context", "--json"],
    ));
    assert_eq!(report["cache_tip"], tip);
    let suggestions = report["suggestions"].as_array().unwrap();
    assert_eq!(suggestions.len(), 2);
    for suggestion in suggestions {
        let path = suggestion["path"].as_str().unwrap();
        assert_ne!(path, "src/a.rs");
        assert_eq!(suggestion["associated_current_paths"][0], "src/a.rs");
        assert!(!suggestion["citations"].as_array().unwrap().is_empty());
        assert!(!suggestion["basis"].as_array().unwrap().is_empty());
        if path == "tests/a.rs" {
            assert_eq!(suggestion["category"], "test");
            assert_eq!(
                suggestion["selection_routes"],
                serde_json::json!(["co_change", "test_path"])
            );
        }
    }
    let text = repo.run(["context"]);
    assert!(text.status.success());
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(text.contains("src/b.rs") && text.contains("tests/a.rs") && text.contains(&tip));
}

#[test]
fn removing_from_index_does_not_erase_net_worktree_identity() {
    let repo = TestRepo::new();
    commit(&repo, &[("src/a.rs", "base\n")], "base");
    git(repo.dir.path(), ["rm", "--cached", "src/a.rs"]);
    let report = json(repo.run(["context", "--json"]));
    assert_eq!(report["input"]["changes"], serde_json::json!([]));
    repo.index();
    let staged = json(repo.run(["context", "--staged", "--json"]));
    assert_eq!(staged["input"]["changes"][0]["status"], "D");
    fs::write(repo.dir.path().join("src/a.rs"), "changed\n").unwrap();
    let net = json(repo.run(["context", "--json"]));
    assert_eq!(net["input"]["changes"].as_array().unwrap().len(), 1);
    assert_eq!(net["input"]["changes"][0]["status"], "M");
}

#[test]
fn staged_paths_rename_and_untracked_exclusion_use_index_only() {
    let repo = TestRepo::new();
    commit(
        &repo,
        &[
            ("old.rs", "base\n"),
            ("removed.rs", "base\n"),
            (".gitignore", "ignored*\n"),
        ],
        "base",
    );
    repo.index();
    git(repo.dir.path(), ["mv", "old.rs", "new.rs"]);
    git(repo.dir.path(), ["rm", "removed.rs"]);
    fs::write(repo.dir.path().join("added.rs"), "added\n").unwrap();
    git(repo.dir.path(), ["add", "added.rs"]);
    fs::write(repo.dir.path().join("loose.rs"), "untracked\n").unwrap();
    fs::write(repo.dir.path().join("ignored.rs"), "ignored\n").unwrap();
    fs::remove_file(repo.dir.path().join("new.rs")).unwrap();
    let before = support::git_stdout(repo.dir.path(), ["ls-files", "--stage"]);
    let staged = json(repo.run(["context", "--staged", "--json"]));
    let changes = staged["input"]["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 3);
    assert!(
        changes
            .iter()
            .any(|c| c["status"] == "A" && c["new_path"] == "added.rs")
    );
    assert!(
        changes
            .iter()
            .any(|c| c["status"] == "D" && c["old_path"] == "removed.rs")
    );
    assert!(
        changes
            .iter()
            .any(|c| c["status"].as_str().unwrap().starts_with('R')
                && c["old_path"] == "old.rs"
                && c["new_path"] == "new.rs")
    );
    let net = json(repo.run(["context", "--json"]));
    let changes = net["input"]["changes"].as_array().unwrap();
    assert!(
        changes
            .iter()
            .any(|c| c["status"] == "untracked" && c["new_path"] == "loose.rs")
    );
    assert!(!changes.iter().any(|c| c["new_path"] == "ignored.rs"));
    assert_eq!(
        before,
        support::git_stdout(repo.dir.path(), ["ls-files", "--stage"])
    );
}

#[test]
fn budgets_order_and_selected_test_exclusions_are_deterministic() {
    let repo = TestRepo::new();
    let files = [
        ("src/a.rs", "base\n"),
        ("src/b.rs", "base\n"),
        ("src/c.rs", "base\n"),
        ("src/d.rs", "base\n"),
        ("src/e.rs", "base\n"),
        ("tests/a.rs", "base\n"),
        ("tests/b.rs", "base\n"),
        ("tests/c.rs", "base\n"),
        ("tests/d.rs", "base\n"),
    ];
    commit(&repo, &files, "together");
    repo.index();
    fs::write(repo.dir.path().join("src/a.rs"), "current\n").unwrap();
    let report = json(repo.run(["context", "--json"]));
    let results = report["suggestions"].as_array().unwrap();
    assert_eq!(results.len(), 6);
    assert_eq!(
        results.iter().filter(|r| r["category"] == "test").count(),
        3
    );
    assert_eq!(
        results
            .iter()
            .filter(|r| r["category"] == "co_changing_file")
            .count(),
        3
    );
    assert_eq!(report["matched_count"], 8);
    assert_eq!(report["truncated"], true);
    assert_eq!(report, json(repo.run(["context", "--json"])));
    let short = json(repo.run(["context", "--limit", "2", "--json"]));
    assert_eq!(short["suggestions"], serde_json::json!(&results[..2]));
    let big = json(repo.run(["context", "--limit", "20", "--json"]));
    assert_eq!(big["suggestions"], report["suggestions"]);
    assert!(!repo.run(["context", "--limit", "0"]).status.success());
    fs::write(repo.dir.path().join("tests/a.rs"), "current test\n").unwrap();
    fs::remove_file(repo.dir.path().join("tests/b.rs")).unwrap();
    let changed = json(repo.run(["context", "--json"]));
    assert!(
        changed["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["path"] != "tests/a.rs"
                && r["path"] != "tests/b.rs"
                && r["path"] != "src/a.rs")
    );
}

#[test]
fn scope_narrows_cached_history_not_current_change_baseline() {
    let repo = TestRepo::new();
    commit(
        &repo,
        &[("src/a.rs", "base\n"), ("src/b.rs", "base\n")],
        "old association",
    );
    let first = repo.head();
    support::git_command(repo.dir.path())
        .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z")
        .args(["commit", "--amend", "--no-edit"])
        .output()
        .unwrap();
    let first_dated = repo.head();
    assert_ne!(first, first_dated);
    commit(
        &repo,
        &[("src/a.rs", "next\n"), ("src/c.rs", "next\n")],
        "new association",
    );
    support::git_command(repo.dir.path())
        .env("GIT_COMMITTER_DATE", "2001-01-01T00:00:00Z")
        .args(["commit", "--amend", "--no-edit"])
        .output()
        .unwrap();
    let cached_tip = repo.head();
    repo.index();
    commit(&repo, &[("src/a.rs", "uncached\n")], "uncached HEAD");
    fs::write(repo.dir.path().join("src/a.rs"), "current\n").unwrap();
    let all = json(repo.run(["context", "--json"]));
    assert_eq!(all["cache_tip"], cached_tip);
    assert_eq!(all["input"]["head"], repo.head());
    assert!(
        all["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("HEAD"))
    );
    let revision = json(repo.run([
        "context",
        "--from-rev",
        &first_dated,
        "--to-rev",
        &cached_tip,
        "--json",
    ]));
    let date = json(repo.run([
        "context",
        "--since",
        "2000-06-01",
        "--until",
        "2001-06-01",
        "--json",
    ]));
    for scoped in [&revision, &date] {
        assert_eq!(scoped["input"], all["input"]);
        assert!(!scoped["scope"].is_null());
        assert_eq!(scoped["suggestions"].as_array().unwrap().len(), 1);
        assert_eq!(scoped["suggestions"][0]["path"], "src/c.rs");
        assert_eq!(scoped["suggestions"][0]["citations"][0]["oid"], cached_tip);
    }
    assert!(
        !repo
            .run(["context", "--from-rev", "not-a-revision"])
            .status
            .success()
    );
    assert!(
        !repo
            .run(["context", "--since", "bad-date"])
            .status
            .success()
    );
    assert!(
        !repo
            .run(["context", "--since", "2002-01-01", "--until", "2001-01-01"])
            .status
            .success()
    );
}

#[test]
fn conflicts_fail_explicitly_without_cache() {
    let repo = TestRepo::new();
    commit(&repo, &[("a.rs", "base\n")], "base");
    git(repo.dir.path(), ["checkout", "-b", "side"]);
    commit(&repo, &[("a.rs", "side\n")], "side");
    git(repo.dir.path(), ["checkout", "main"]);
    commit(&repo, &[("a.rs", "main\n")], "main");
    let merge = support::git_command(repo.dir.path())
        .args(["merge", "side"])
        .output()
        .unwrap();
    assert!(!merge.status.success());
    for args in [vec!["context"], vec!["context", "--staged"]] {
        let output = repo.run(args);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("unresolved conflicts"));
    }
    assert!(!repo.dir.path().join(".gitscry").exists());
}

#[test]
fn staged_context_omits_historical_tests_missing_from_worktree() {
    let repo = TestRepo::new();
    commit(
        &repo,
        &[("src/a.rs", "base\n"), ("tests/a.rs", "base\n")],
        "association",
    );
    repo.index();
    fs::write(repo.dir.path().join("src/a.rs"), "staged\n").unwrap();
    git(repo.dir.path(), ["add", "src/a.rs"]);
    fs::remove_file(repo.dir.path().join("tests/a.rs")).unwrap();
    let report = json(repo.run(["context", "--staged", "--json"]));
    assert_eq!(report["suggestions"], serde_json::json!([]));
    assert!(
        report["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("safe regular files"))
    );
}

#[test]
fn detected_rename_sides_are_excluded_from_associations() {
    let repo = TestRepo::new();
    commit(
        &repo,
        &[
            ("old.rs", "unique old\n"),
            ("new.rs", "different new\n"),
            ("other.rs", "other\n"),
        ],
        "association",
    );
    git(repo.dir.path(), ["rm", "new.rs"]);
    git(repo.dir.path(), ["commit", "-m", "remove destination"]);
    repo.index();
    git(repo.dir.path(), ["mv", "old.rs", "new.rs"]);
    let report = json(repo.run(["context", "--staged", "--json"]));
    assert_eq!(report["input"]["changes"].as_array().unwrap().len(), 1);
    assert_eq!(report["input"]["changes"][0]["old_path"], "old.rs");
    assert_eq!(report["input"]["changes"][0]["new_path"], "new.rs");
    assert!(
        report["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["path"] != "old.rs" && r["path"] != "new.rs")
    );
}

#[cfg(unix)]
#[test]
fn case_distinct_paths_and_symlink_tests_keep_safe_identities() {
    use std::os::unix::fs::symlink;
    let repo = TestRepo::new();
    commit(
        &repo,
        &[
            ("src/A.rs", "upper\n"),
            ("src/a.rs", "lower\n"),
            ("tests/a.rs", "test\n"),
        ],
        "association",
    );
    repo.index();
    let external = tempfile::tempdir().unwrap();
    let target = external.path().join("test.rs");
    fs::write(&target, "external\n").unwrap();
    fs::remove_file(repo.dir.path().join("tests/a.rs")).unwrap();
    symlink(&target, repo.dir.path().join("tests/a.rs")).unwrap();
    fs::write(repo.dir.path().join("src/A.rs"), "selected\n").unwrap();
    git(repo.dir.path(), ["add", "src/A.rs"]);
    let report = json(repo.run(["context", "--staged", "--json"]));
    let suggestions = report["suggestions"].as_array().unwrap();
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0]["path"], "src/a.rs");
    assert_eq!(
        suggestions[0]["associated_current_paths"],
        serde_json::json!(["src/A.rs"])
    );
    assert_eq!(fs::read_to_string(target).unwrap(), "external\n");
}

#[test]
fn submodule_paths_are_selected_without_submodule_file_changes() {
    let source = TestRepo::new();
    commit(&source, &[("inside.rs", "base\n")], "submodule base");
    let repo = TestRepo::new();
    let source_path = source.dir.path().to_str().unwrap();
    git(
        repo.dir.path(),
        [
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            source_path,
            "vendor",
        ],
    );
    git(repo.dir.path(), ["commit", "-m", "add submodule"]);
    repo.index();
    fs::write(repo.dir.path().join("vendor/inside.rs"), "dirty content\n").unwrap();
    let unchanged = json(repo.run(["context", "--json"]));
    assert_eq!(unchanged["input"]["changes"], serde_json::json!([]));
    commit(&source, &[("inside.rs", "next\n")], "new submodule tip");
    let tip = source.head();
    git(&repo.dir.path().join("vendor"), ["fetch"]);
    git(
        &repo.dir.path().join("vendor"),
        ["checkout", "--force", &tip],
    );
    let net = json(repo.run(["context", "--json"]));
    assert_eq!(net["input"]["changes"].as_array().unwrap().len(), 1);
    assert_eq!(net["input"]["changes"][0]["new_path"], "vendor");
    git(repo.dir.path(), ["add", "vendor"]);
    let staged = json(repo.run(["context", "--staged", "--json"]));
    assert_eq!(staged["input"]["changes"], net["input"]["changes"]);
}

#[test]
fn clean_unborn_and_untracked_inputs_do_not_initialize_cache() {
    let repo = TestRepo::new();
    for args in [
        vec!["context", "--json"],
        vec!["context", "--staged", "--json"],
    ] {
        let report = json(repo.run(args));
        assert_eq!(report["input"]["changes"], serde_json::json!([]));
        assert!(report["input"]["head"].is_null());
    }
    fs::write(repo.dir.path().join("new.rs"), "new\n").unwrap();
    let output = repo.run(["context"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("gitscry index"));
    assert!(!repo.dir.path().join(".gitscry").exists());
    let staged = json(repo.run(["context", "--staged", "--json"]));
    assert_eq!(staged["input"]["changes"], serde_json::json!([]));
}
