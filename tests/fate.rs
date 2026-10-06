mod support;

use std::{fs, path::Path};

use serde_json::Value;
use support::{TestRepo, git, git_command};

fn commit(repo: &TestRepo, path: &str, contents: &str, subject: &str, date: &str) -> String {
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(repo.dir.path().join(parent)).expect("create parent directory");
    }
    fs::write(repo.dir.path().join(path), contents).expect("write tracked file");
    git(repo.dir.path(), ["add", "--all"]);
    let output = git_command(repo.dir.path())
        .args(["commit", "-m", subject])
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .expect("run git commit");
    assert!(
        output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    repo.head()
}

fn rename(repo: &TestRepo, old: &str, new: &str, subject: &str, date: &str) -> String {
    fs::rename(repo.dir.path().join(old), repo.dir.path().join(new)).expect("rename tracked file");
    git(repo.dir.path(), ["add", "--all"]);
    let output = git_command(repo.dir.path())
        .args(["commit", "-m", subject])
        .env("GIT_AUTHOR_DATE", date)
        .output()
        .expect("run git rename commit");
    assert!(
        output.status.success(),
        "git rename failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    repo.head()
}

fn delete(repo: &TestRepo, path: &str, subject: &str, date: &str) -> String {
    fs::remove_file(repo.dir.path().join(path)).expect("remove tracked file");
    git(repo.dir.path(), ["add", "--all"]);
    let output = git_command(repo.dir.path())
        .args(["commit", "-m", subject])
        .env("GIT_AUTHOR_DATE", date)
        .output()
        .expect("run git delete commit");
    assert!(
        output.status.success(),
        "git delete failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    repo.head()
}

fn json(repo: &TestRepo, args: &[&str]) -> Value {
    let output = repo.run(args);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("valid fate JSON")
}

#[test]
fn fate_requires_exactly_one_selector_and_at() {
    let repo = TestRepo::new();
    commit(&repo, "a.txt", "one\n", "Add", "2020-01-01T00:00:00Z");
    repo.index();

    // --at is required.
    let missing_at = repo.run(["fate", "a.txt", "--line", "1"]);
    assert_ne!(missing_at.status.code(), Some(0));

    // --symbol is not part of this slice.
    let symbol = repo.run(["fate", "a.txt", "--symbol", "x", "--at", "HEAD"]);
    assert_ne!(symbol.status.code(), Some(0));
}

#[test]
fn fate_validates_path_line_and_ancestry() {
    let repo = TestRepo::new();
    let first = commit(&repo, "a.txt", "one\n", "Add", "2020-01-01T00:00:00Z");
    let _second = commit(
        &repo,
        "a.txt",
        "one\ntwo\n",
        "Extend",
        "2020-01-02T00:00:00Z",
    );
    repo.index();

    let out = repo.run([
        "fate",
        "missing.txt",
        "--line",
        "1",
        "--at",
        "HEAD",
        "--json",
    ]);
    assert_ne!(out.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .to_lowercase()
            .contains("path"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = repo.run(["fate", "a.txt", "--line", "99", "--at", "HEAD", "--json"]);
    assert_ne!(out.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("99"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Start must be an ancestor of the endpoint.
    let out = repo.run([
        "fate", "a.txt", "--line", "1", "--at", "HEAD", "--to-rev", &first, "--json",
    ]);
    assert_ne!(out.status.code(), Some(0));
}

#[test]
fn fate_equal_endpoints_reports_validated_location_without_events() {
    let repo = TestRepo::new();
    let start = commit(&repo, "a.txt", "one\ntwo\n", "Add", "2020-01-01T00:00:00Z");
    repo.index();
    let report = json(
        &repo,
        &["fate", "a.txt", "--line", "2", "--at", &start, "--json"],
    );
    assert_eq!(report["kind"], "fate");
    assert_eq!(report["start_revision"], start);
    assert_eq!(report["endpoint"], start);
    assert_eq!(report["final_state"], "reached_endpoint");
    assert_eq!(report["events"].as_array().unwrap().len(), 0);
    let location = &report["last_location"];
    assert_eq!(location["path"], "a.txt");
    assert_eq!(location["line"], 2);
}

#[test]
fn fate_maps_lines_through_unrelated_edits_without_events() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "alpha\nbeta\ngamma\n",
        "Add",
        "2020-01-01T00:00:00Z",
    );
    commit(
        &repo,
        "other.txt",
        "noise\n",
        "Unrelated",
        "2020-01-02T00:00:00Z",
    );
    // Insertions before the target line shift it; the line itself is untouched.
    commit(
        &repo,
        "a.txt",
        "zero\nalpha\nbeta\ngamma\n",
        "Insert before",
        "2020-01-03T00:00:00Z",
    );
    commit(
        &repo,
        "a.txt",
        "zero\nalpha\nbeta\ngamma\ndelta\n",
        "Insert after",
        "2020-01-04T00:00:00Z",
    );
    repo.index();
    let report = json(
        &repo,
        &["fate", "a.txt", "--line", "2", "--at", &start, "--json"],
    );
    assert_eq!(report["final_state"], "reached_endpoint");
    assert_eq!(report["events"].as_array().unwrap().len(), 0);
    let location = &report["last_location"];
    assert_eq!(location["path"], "a.txt");
    assert_eq!(location["line"], 3);
}

#[test]
fn fate_reports_reached_endpoint_with_events() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "alpha\nbeta\n",
        "Add",
        "2020-01-01T00:00:00Z",
    );
    // Rewrite beta in place (one deletion, one addition).
    commit(
        &repo,
        "a.txt",
        "alpha\nBETA\n",
        "Rewrite beta",
        "2020-01-02T00:00:00Z",
    );
    repo.index();
    let report = json(
        &repo,
        &["fate", "a.txt", "--line", "2", "--at", &start, "--json"],
    );
    assert_eq!(report["final_state"], "reached_endpoint");
    let events = report["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["relationship"], "rewrite");
    assert_eq!(events[0]["before"]["path"], "a.txt");
    assert_eq!(events[0]["before"]["line"], 2);
    assert_eq!(events[0]["after"]["path"], "a.txt");
    assert_eq!(events[0]["after"]["line"], 2);
    assert_eq!(report["last_location"]["line"], 2);
}

#[test]
fn fate_survives_renames_and_coordinates() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "old.rs",
        "keep\nmove\n",
        "Add",
        "2020-01-01T00:00:00Z",
    );
    rename(&repo, "old.rs", "new.rs", "Rename", "2020-01-02T00:00:00Z");
    commit(
        &repo,
        "new.rs",
        "top\nkeep\nmove\n",
        "Insert before",
        "2020-01-03T00:00:00Z",
    );
    repo.index();
    let report = json(
        &repo,
        &["fate", "old.rs", "--line", "2", "--at", &start, "--json"],
    );
    assert_eq!(report["final_state"], "reached_endpoint");
    assert_eq!(report["events"].as_array().unwrap().len(), 0);
    let location = &report["last_location"];
    assert_eq!(location["path"], "new.rs");
    assert_eq!(location["line"], 3);
}

#[test]
fn fate_reports_deleted_without_claiming_survival() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "alpha\nbeta\n",
        "Add",
        "2020-01-01T00:00:00Z",
    );
    let removal = delete(&repo, "a.txt", "Delete file", "2020-01-02T00:00:00Z");
    commit(
        &repo,
        "other.txt",
        "after\n",
        "Later",
        "2020-01-03T00:00:00Z",
    );
    repo.index();
    let report = json(
        &repo,
        &["fate", "a.txt", "--line", "2", "--at", &start, "--json"],
    );
    assert_eq!(report["final_state"], "deleted");
    assert_eq!(report["stopped_at"], removal);
    assert_eq!(report["events"].as_array().unwrap().len(), 1);
    let location = &report["last_location"];
    assert_eq!(location["path"], "a.txt");
    assert_eq!(location["line"], 2);
}

#[test]
fn fate_does_not_cross_file_recreation() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "alpha\nbeta\n",
        "Add",
        "2020-01-01T00:00:00Z",
    );
    let removal = delete(&repo, "a.txt", "Delete file", "2020-01-02T00:00:00Z");
    // Recreation at the same path is a new incarnation; never resumed.
    commit(
        &repo,
        "a.txt",
        "totally\nnew\n",
        "Recreate",
        "2020-01-03T00:00:00Z",
    );
    repo.index();
    let report = json(
        &repo,
        &["fate", "a.txt", "--line", "2", "--at", &start, "--json"],
    );
    assert_eq!(report["final_state"], "deleted");
    assert_eq!(report["stopped_at"], removal);
}

#[test]
fn fate_reports_unknown_on_incomplete_coverage() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "alpha\nbeta\n",
        "Add",
        "2020-01-01T00:00:00Z",
    );
    repo.index();
    // Create endpoint history that the cache has not seen: shallow simulation via
    // a commit added after indexing, reachable only from a new branch, with the
    // endpoint pinned there.
    git(repo.dir.path(), ["checkout", "-b", "side"]);
    let _side_tip = commit(
        &repo,
        "a.txt",
        "alpha\nBETA\n",
        "Side rewrite",
        "2020-01-02T00:00:00Z",
    );
    let output = repo.run([
        "fate", "a.txt", "--line", "2", "--at", &start, "--to-rev", "side", "--json",
    ]);
    // The side tip is not in the published cache generation; an explicit
    // endpoint outside the cache is an error, not empty success.
    assert_ne!(output.status.code(), Some(0));
}

#[test]
fn fate_max_commits_bounds_inspection_and_reports_incomplete() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "alpha\nbeta\n",
        "Add",
        "2020-01-01T00:00:00Z",
    );
    commit(
        &repo,
        "a.txt",
        "alpha\nBETA\n",
        "Rewrite",
        "2020-01-02T00:00:00Z",
    );
    commit(
        &repo,
        "a.txt",
        "alpha\nGAMMA\n",
        "Rewrite again",
        "2020-01-03T00:00:00Z",
    );
    repo.index();
    let report = json(
        &repo,
        &[
            "fate",
            "a.txt",
            "--line",
            "2",
            "--at",
            &start,
            "--max-commits",
            "1",
            "--json",
        ],
    );
    // Budget exhausted before the endpoint: state must be unknown, never survival.
    assert_eq!(report["final_state"], "unknown");
    assert_eq!(report["inspected_commits"], 1);
    assert_eq!(report["events"].as_array().unwrap().len(), 1);
    assert!(
        report["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap_or("").contains("max-commits")),
        "{report}"
    );
}

#[test]
fn fate_limit_defaults_to_last_20_and_keeps_summary() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "alpha\nbeta\n",
        "Add",
        "2020-01-01T00:00:00Z",
    );
    // 25 rewrites of the target line.
    for index in 0..25 {
        commit(
            &repo,
            "a.txt",
            &format!("alpha\nv{index:02}\n"),
            &format!("Rewrite {index}"),
            &format!("2020-01-{:02}T00:00:00Z", index + 2),
        );
    }
    repo.index();
    let report = json(
        &repo,
        &["fate", "a.txt", "--line", "2", "--at", &start, "--json"],
    );
    assert_eq!(report["final_state"], "reached_endpoint");
    let events = report["events"].as_array().unwrap();
    assert_eq!(events.len(), 20);
    // Display limit never fabricates a direct relationship; total is exposed.
    assert_eq!(report["total_events"], 25);
    assert_eq!(report["display_truncated"], true);
    assert_eq!(report["limit"], 20);
}

#[test]
fn fate_final_state_is_invariant_under_display_limit() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "alpha\nbeta\n",
        "Add",
        "2020-01-01T00:00:00Z",
    );
    let removal = delete(&repo, "a.txt", "Delete file", "2020-01-02T00:00:00Z");
    commit(
        &repo,
        "other.txt",
        "after\n",
        "Later",
        "2020-01-03T00:00:00Z",
    );
    repo.index();
    let report = json(
        &repo,
        &[
            "fate", "a.txt", "--line", "2", "--at", &start, "--limit", "1", "--json",
        ],
    );
    assert_eq!(report["final_state"], "deleted");
    assert_eq!(report["stopped_at"], removal);
    assert_eq!(report["events"].as_array().unwrap().len(), 1);
}

#[test]
fn fate_text_output_carries_summary() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "alpha\nbeta\n",
        "Add",
        "2020-01-01T00:00:00Z",
    );
    commit(
        &repo,
        "a.txt",
        "alpha\nBETA\n",
        "Rewrite",
        "2020-01-02T00:00:00Z",
    );
    repo.index();
    let output = repo.run(["fate", "a.txt", "--line", "2", "--at", &start]);
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("reached_endpoint"), "{text}");
    assert!(text.contains(&start), "{text}");
}
