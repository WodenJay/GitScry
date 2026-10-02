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
