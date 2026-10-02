mod support;

use serde_json::Value;
use std::fs;
use support::{TestRepo, git, git_command};

fn commit(repo: &TestRepo, files: &[(&str, &str)], message: &str, date: &str) -> String {
    for (path, text) in files {
        fs::write(repo.dir.path().join(path), text).unwrap();
    }
    git(repo.dir.path(), ["add", "--all"]);
    let output = git_command(repo.dir.path())
        .args(["commit", "-m", message])
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    repo.head()
}

fn query(repo: &TestRepo, args: &[&str]) -> Value {
    let mut command = vec!["trace-removal", "--code", "old()", "--json"];
    command.extend_from_slice(args);
    let output = repo.run(command);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn groups_deleted_lines_into_events_with_complete_historical_locators() {
    let repo = TestRepo::new();
    let parent = commit(
        &repo,
        &[
            ("a.rs", "old() old()\nkeep\nold()\nOld()\n"),
            ("b.rs", "old()\n"),
        ],
        "Introduce",
        "2000-01-01T00:00:00Z",
    );
    let removal = commit(
        &repo,
        &[("a.rs", "keep\nOld()\nnew()\n"), ("b.rs", "new()\n")],
        "Remove\n\nA complete explanation.\n\nMore detail.",
        "2000-01-02T00:00:00Z",
    );
    repo.index();
    let report = query(&repo, &["--limit", "1"]);
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["kind"], "trace-removal");
    assert_eq!(report["matched_count"], 2);
    assert_eq!(report["truncated"], true);
    assert_eq!(report["events"].as_array().unwrap().len(), 1);
    let event = &report["events"][0];
    assert_eq!(event["commit_id"], removal);
    assert_eq!(event["timestamp"], "2000-01-02T00:00:00Z");
    assert_eq!(
        event["message"],
        "Remove\n\nA complete explanation.\n\nMore detail.\n"
    );
    assert_eq!(event["first_parent_id"], parent);
    assert_eq!(event["old_path"], "a.rs");
    assert_eq!(event["file_change"]["status"], "M");
    assert_eq!(
        event["matches"],
        serde_json::json!([
            {"line_number": 1, "line": "old() old()"}, {"line_number": 3, "line": "old()"}
        ])
    );
    let human = repo.run(["trace-removal", "--code", "old()", "--limit", "1"]);
    assert!(human.status.success());
    let human = String::from_utf8(human.stdout).unwrap();
    assert!(human.contains(&removal));
    assert!(human.contains(&parent));
    assert!(human.contains("More detail."));
}

#[test]
fn scopes_before_limiting_and_preserves_repeated_deletions_and_cache_boundary() {
    let repo = TestRepo::new();
    let root = commit(
        &repo,
        &[("a.rs", "old()\n")],
        "Root",
        "2000-01-01T00:00:00Z",
    );
    let first = commit(
        &repo,
        &[("a.rs", "new()\n")],
        "First removal",
        "2000-01-02T00:00:00Z",
    );
    commit(
        &repo,
        &[("a.rs", "old()\n"), ("b.rs", "old()\n")],
        "Restore elsewhere too",
        "2000-01-03T00:00:00Z",
    );
    let second = commit(
        &repo,
        &[("a.rs", "new()\n")],
        "Second removal",
        "2000-01-04T00:00:00Z",
    );
    repo.index();
    let report = query(&repo, &[]);
    assert_eq!(report["matched_count"], 2);
    assert_eq!(report["events"][0]["commit_id"], second);
    assert_eq!(report["events"][1]["commit_id"], first);
    let scoped = query(&repo, &["--to-rev", &first, "--limit", "1"]);
    assert_eq!(scoped["matched_count"], 1);
    assert_eq!(scoped["events"][0]["commit_id"], first);
    assert_eq!(scoped["scope"]["to_rev"], first);
    assert_eq!(query(&repo, &["--to-rev", &root])["matched_count"], 0);
    assert_eq!(query(&repo, &["--path", "b.rs"])["matched_count"], 0);
    commit(
        &repo,
        &[("b.rs", "new()\n")],
        "Uncached removal",
        "2000-01-05T00:00:00Z",
    );
    assert_eq!(query(&repo, &[]), report);
}

#[test]
fn rejects_invalid_queries_and_execution_errors_but_empty_results_succeed() {
    let repo = TestRepo::new();
    for args in [
        vec!["trace-removal"],
        vec!["trace-removal", "--code", ""],
        vec!["trace-removal", "--code", "a\nb"],
        vec!["trace-removal", "--code", "a\rb"],
        vec!["trace-removal", "--code", "a", "--limit", "0"],
    ] {
        assert_eq!(repo.run(args).status.code(), Some(2));
    }
    assert_eq!(
        repo.run(["trace-removal", "--code", "a"]).status.code(),
        Some(1)
    );
    commit(
        &repo,
        &[("a.rs", "old()\n")],
        "Root",
        "2000-01-01T00:00:00Z",
    );
    repo.index();
    assert_eq!(query(&repo, &[])["events"], serde_json::json!([]));
    let empty = repo.run(["trace-removal", "--code", "old()"]);
    assert!(empty.status.success());
    assert!(String::from_utf8_lossy(&empty.stdout).contains("No matching deletion events"));
    assert_eq!(
        repo.run(["trace-removal", "--code", "a", "--to-rev", "missing"])
            .status
            .code(),
        Some(2)
    );
    fs::write(repo.dir.path().join(".gitscry/cache.sqlite"), b"damaged").unwrap();
    assert_eq!(
        repo.run(["trace-removal", "--code", "a"]).status.code(),
        Some(1)
    );
}

#[test]
fn literal_matching_excludes_context_additions_and_case_variants_across_hunks() {
    let repo = TestRepo::new();
    let mut old = vec!["padding"; 30];
    old[0] = "old()";
    old[15] = "old()";
    old[29] = "old() old()";
    commit(
        &repo,
        &[("a.rs", &old.join("\n"))],
        "Root",
        "2000-01-01T00:00:00Z",
    );
    let mut new = old.clone();
    new[0] = "Old()";
    new[29] = "new()";
    commit(
        &repo,
        &[("a.rs", &new.join("\n")), ("b.rs", "old()\n")],
        "Remove two hunks",
        "2000-01-02T00:00:00Z",
    );
    repo.index();
    let report = query(&repo, &[]);
    assert_eq!(report["matched_count"], 1);
    assert_eq!(
        report["events"][0]["matches"],
        serde_json::json!([
            {"line_number": 1, "line": "old()"}, {"line_number": 30, "line": "old() old()"}
        ])
    );
    let literal = repo.run(["trace-removal", "--code", "old() old()", "--json"]);
    let literal: Value = serde_json::from_slice(&literal.stdout).unwrap();
    assert_eq!(literal["events"][0]["matches"].as_array().unwrap().len(), 1);
    let absent = repo.run(["trace-removal", "--code", "old.*", "--json"]);
    assert!(absent.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&absent.stdout).unwrap()["matched_count"],
        0
    );
}

#[test]
fn equal_timestamps_use_commit_then_path_ties_and_dates_filter_before_limit() {
    let repo = TestRepo::new();
    let root = commit(
        &repo,
        &[("a.rs", "old()\n"), ("b.rs", "old()\n")],
        "Root",
        "2000-01-01T00:00:00Z",
    );
    let first = commit(
        &repo,
        &[("a.rs", "new()\n"), ("b.rs", "new()\n")],
        "First removal",
        "2000-01-02T00:00:00Z",
    );
    commit(
        &repo,
        &[("a.rs", "old()\n")],
        "Restore",
        "2000-01-02T00:00:00Z",
    );
    let second = commit(
        &repo,
        &[("a.rs", "new()\n")],
        "Second removal",
        "2000-01-02T00:00:00Z",
    );
    repo.index();
    let report = query(&repo, &[]);
    let mut expected = vec![
        (first.clone(), "a.rs"),
        (first.clone(), "b.rs"),
        (second, "a.rs"),
    ];
    expected.sort();
    for (event, (id, path)) in report["events"].as_array().unwrap().iter().zip(expected) {
        assert_eq!(event["commit_id"], id);
        assert_eq!(event["old_path"], path);
    }
    assert_eq!(
        query(&repo, &["--from-rev", &first, "--limit", "1"])["matched_count"],
        1
    );
    assert_eq!(
        query(&repo, &["--from-rev", &root, "--until", "2000-01-01"])["matched_count"],
        0
    );
    assert_eq!(
        query(
            &repo,
            &[
                "--since",
                "2000-01-02",
                "--until",
                "2000-01-02",
                "--limit",
                "1"
            ]
        )["matched_count"],
        3
    );
}

#[test]
fn reports_detected_rename_old_paths_and_whole_file_deletions_not_pure_moves() {
    let repo = TestRepo::new();
    let original = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nold()\n";
    let parent = commit(
        &repo,
        &[("old.rs", original), ("deleted.rs", "old()\n")],
        "Root",
        "2000-01-01T00:00:00Z",
    );
    git(repo.dir.path(), ["mv", "old.rs", "new.rs"]);
    fs::remove_file(repo.dir.path().join("deleted.rs")).unwrap();
    commit(
        &repo,
        &[(
            "new.rs",
            "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnew()\n",
        )],
        "Rename with edit and delete",
        "2000-01-02T00:00:00Z",
    );
    git(repo.dir.path(), ["mv", "new.rs", "moved.rs"]);
    commit(&repo, &[], "Pure move", "2000-01-03T00:00:00Z");
    repo.index();
    let report = query(&repo, &["--path", "old.rs"]);
    assert_eq!(report["matched_count"], 1);
    assert_eq!(report["events"][0]["first_parent_id"], parent);
    let change = &report["events"][0]["file_change"];
    assert!(change["status"].as_str().unwrap().starts_with('R'));
    assert_eq!(change["old_path"], "old.rs");
    assert_eq!(change["new_path"], "new.rs");
    assert_eq!(query(&repo, &["--path", "new.rs"])["matched_count"], 0);
    assert_eq!(
        query(&repo, &["--path", "deleted.rs"])["events"][0]["file_change"]["status"],
        "D"
    );
    assert_eq!(query(&repo, &[])["matched_count"], 2);
}

#[test]
fn merge_events_compare_with_the_first_parent_not_other_parents() {
    let repo = TestRepo::new();
    let root = commit(
        &repo,
        &[("a.rs", "old()\n")],
        "Root",
        "2000-01-01T00:00:00Z",
    );
    git(repo.dir.path(), ["checkout", "-b", "side"]);
    let side = commit(
        &repo,
        &[("a.rs", "new()\n")],
        "Side removal",
        "2000-01-02T00:00:00Z",
    );
    git(repo.dir.path(), ["checkout", "main"]);
    git(
        repo.dir.path(),
        ["merge", "--no-ff", "side", "-m", "Merge removal"],
    );
    let merge = repo.head();
    repo.index();
    let scoped = query(&repo, &["--from-rev", &side]);
    assert_eq!(scoped["matched_count"], 1);
    assert_eq!(scoped["events"][0]["commit_id"], merge);
    assert_eq!(scoped["events"][0]["first_parent_id"], root);
    // Merging a branch that retains old() must not invent another deletion.
    git(repo.dir.path(), ["checkout", "-b", "retaining", &root]);
    commit(
        &repo,
        &[("unrelated", "change")],
        "Keep old text",
        "2000-01-03T00:00:00Z",
    );
    git(repo.dir.path(), ["checkout", "main"]);
    git(
        repo.dir.path(),
        ["merge", "--no-ff", "retaining", "-m", "Merge retained text"],
    );
    repo.index();
    assert_eq!(query(&repo, &[])["matched_count"], 2);
}

#[test]
fn shallow_coverage_stays_visible_with_usable_parent_locators() {
    let source = TestRepo::new();
    let root = commit(
        &source,
        &[("a.rs", "old()\n")],
        "Root",
        "2000-01-01T00:00:00Z",
    );
    commit(
        &source,
        &[("a.rs", "new()\n")],
        "Remove",
        "2000-01-02T00:00:00Z",
    );
    let holder = tempfile::tempdir().unwrap();
    let clone = holder.path().join("clone");
    let cloned = git_command(holder.path())
        .args(["clone", "--depth", "2", "--no-local"])
        .arg(source.dir.path())
        .arg(&clone)
        .output()
        .unwrap();
    assert!(cloned.status.success());
    assert!(TestRepo::run_at(&clone, ["index"]).status.success());
    let output = TestRepo::run_at(&clone, ["trace-removal", "--code", "old()", "--json"]);
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["matched_count"], 1);
    assert_eq!(report["events"][0]["first_parent_id"], root);
    assert!(
        report["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("shallow"))
    );
    let human = TestRepo::run_at(&clone, ["trace-removal", "--code", "old()"]);
    assert!(human.status.success());
    assert!(String::from_utf8_lossy(&human.stderr).contains("shallow"));
}

#[test]
fn command_help_and_json_contract_are_explicit_and_extensible() {
    let repo = TestRepo::new();
    let help = repo.run(["trace-removal", "--help"]);
    assert!(help.status.success());
    let text = String::from_utf8(help.stdout).unwrap();
    for option in [
        "--code",
        "--path",
        "--from-rev",
        "--to-rev",
        "--since",
        "--until",
        "--limit",
        "--json",
    ] {
        assert!(text.contains(option), "{option}");
    }
    assert!(text.contains("[default: 10]"));
    assert!(text.contains("first parent"));
    assert!(text.contains("not a resolved symbol"));
    assert!(String::from_utf8_lossy(&repo.run(["--help"]).stdout).contains("trace-removal"));
    commit(
        &repo,
        &[("a.rs", "old()\n")],
        "Root",
        "2000-01-01T00:00:00Z",
    );
    repo.index();
    let report = query(&repo, &[]);
    assert_eq!(
        report["query"],
        serde_json::json!({"code": "old()", "path": null})
    );
    assert_eq!(report["cache_tip"], repo.head());
    assert_eq!(report["limit"], 10);
    assert!(report["warnings"].is_array());
    assert!(report["notices"].is_array());
    assert_eq!(report["truncated"], false);
}
