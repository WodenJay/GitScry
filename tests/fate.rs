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
        .env("GIT_COMMITTER_DATE", date)
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

    let missing_selector = repo.run(["fate", "a.txt", "--at", "HEAD"]);
    assert_eq!(missing_selector.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&missing_selector.stderr);
    assert!(
        stderr.contains("exactly one of --line or --symbol"),
        "{stderr}"
    );
    // Missing symbol targets must be rejected.
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
    assert_eq!(report["schema_version"], 3);
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
fn fate_stops_as_unknown_after_line_rewrite() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "alpha\nbeta\n",
        "Add",
        "2020-01-01T00:00:00Z",
    );
    // Rewrite beta in place (one deletion, one addition).
    let rewrite = commit(
        &repo,
        "a.txt",
        "alpha\nBETA\n",
        "Rewrite beta",
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
            "fate", "a.txt", "--line", "2", "--at", &start, "--patch", "--json",
        ],
    );
    assert_eq!(report["final_state"], "unknown");
    assert_eq!(report["stopped_at"], rewrite);
    assert_eq!(report["stop_reason"]["code"], "line_rewritten");
    let events = report["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    let patch = &events[0]["patch"];
    assert_eq!(patch["status"], "available");
    assert!(
        patch["hunks"][0]["text"]
            .as_str()
            .unwrap()
            .contains("-beta")
    );
    assert_eq!(events[0]["relationship"], "rewrite");
    assert_eq!(events[0]["before"]["path"], "a.txt");
    assert_eq!(events[0]["before"]["line"], 2);
    assert_eq!(events[0]["after"]["path"], "a.txt");
    assert_eq!(events[0]["after"]["line"], 2);
    assert_eq!(report["last_location"]["line"], 2);
    assert_eq!(report["associations"][0]["path"], "a.txt");
    assert_eq!(report["associations"][0]["line"], 2);
    assert_eq!(report["schema_version"], 3);
    let text_output = repo.run(["fate", "a.txt", "--line", "2", "--at", &start, "--patch"]);
    assert_eq!(text_output.status.code(), Some(0));
    let text = String::from_utf8_lossy(&text_output.stdout);
    assert!(text.contains("patch excerpt: available"), "{text}");
    assert!(text.contains("-beta"), "{text}");
}

#[test]
fn fate_line_split_reports_rewrite_candidates() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "before\nbeta\nafter\n",
        "Create target",
        "2020-01-01T00:00:00Z",
    );
    let split = commit(
        &repo,
        "a.txt",
        "before\nbeta left\nbeta right\nafter\n",
        "Split target line",
        "2020-01-02T00:00:00Z",
    );
    repo.index();
    let report = json(
        &repo,
        &["fate", "a.txt", "--line", "2", "--at", &start, "--json"],
    );
    assert_eq!(report["final_state"], "unknown", "{report}");
    assert_eq!(report["stopped_at"], split);
    assert_eq!(report["stop_reason"]["code"], "line_rewritten");
    let associations = report["associations"].as_array().unwrap();
    assert_eq!(associations.len(), 2);
    assert_eq!(associations[0]["line"], 2);
    assert_eq!(associations[1]["line"], 3);
}

#[test]
fn fate_line_merge_reports_rewrite_candidate() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "before\nbeta left\nbeta right\nafter\n",
        "Create target lines",
        "2020-01-01T00:00:00Z",
    );
    let merged = commit(
        &repo,
        "a.txt",
        "before\nbeta merged\nafter\n",
        "Merge target lines",
        "2020-01-02T00:00:00Z",
    );
    repo.index();
    let report = json(
        &repo,
        &["fate", "a.txt", "--line", "2", "--at", &start, "--json"],
    );
    assert_eq!(report["final_state"], "unknown", "{report}");
    assert_eq!(report["stopped_at"], merged);
    assert_eq!(report["stop_reason"]["code"], "line_rewritten");
    assert_eq!(report["associations"][0]["line"], 2);
}

#[test]
fn fate_reports_line_deletion_when_file_remains() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "alpha\nbeta\ngamma\n",
        "Add",
        "2020-01-01T00:00:00Z",
    );
    let removal = commit(
        &repo,
        "a.txt",
        "alpha\ngamma\n",
        "Delete beta",
        "2020-01-02T00:00:00Z",
    );
    repo.index();
    let report = json(
        &repo,
        &["fate", "a.txt", "--line", "2", "--at", &start, "--json"],
    );
    assert_eq!(report["final_state"], "deleted");
    assert_eq!(report["stopped_at"], removal);
    assert_eq!(report["stop_reason"]["code"], "removed");
    assert_eq!(report["events"].as_array().unwrap().len(), 1);
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
    let events = report["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["relationship"], "rename");
    assert_eq!(events[0]["before"]["path"], "old.rs");
    assert_eq!(events[0]["after"]["path"], "new.rs");
    assert_eq!(events[0]["after"]["line"], 2);
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
    git(repo.dir.path(), ["checkout", "main"]);
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
    rename(
        &repo,
        "a.txt",
        "b.txt",
        "Rename one",
        "2020-01-02T00:00:00Z",
    );
    rename(
        &repo,
        "b.txt",
        "c.txt",
        "Rename two",
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
fn fate_merge_inspection_respects_the_global_commit_budget() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "before\ntracked\nafter\n",
        "Add tracked line",
        "2020-01-01T00:00:00Z",
    );
    git(repo.dir.path(), ["switch", "-c", "side"]);
    commit(
        &repo,
        "a.txt",
        "side\nbefore\ntracked\nafter\n",
        "Shift tracked line",
        "2020-01-02T00:00:00Z",
    );
    git(repo.dir.path(), ["switch", "-"]);
    commit(
        &repo,
        "main.txt",
        "main\n",
        "Main change",
        "2020-01-03T00:00:00Z",
    );
    let merge = git_command(repo.dir.path())
        .args(["merge", "--no-ff", "--no-edit", "side"])
        .env("GIT_AUTHOR_DATE", "2020-01-04T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-04T00:00:00Z")
        .output()
        .expect("merge branches");
    assert!(
        merge.status.success(),
        "{}",
        String::from_utf8_lossy(&merge.stderr)
    );
    let endpoint = repo.head();
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
            "--to-rev",
            &endpoint,
            "--max-commits",
            "2",
            "--json",
        ],
    );

    assert_eq!(report["final_state"], "unknown");
    assert_eq!(report["inspected_commits"], 2);
    assert_eq!(report["stop_reason"]["code"], "max_commits_exhausted");
    assert!(
        report["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|event| event["commit_id"] != endpoint)
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
    // 25 file renames preserve the tracked line and produce 25 events.
    let mut path = "a.txt".to_owned();
    for index in 0..25 {
        let next_path = format!("a{}.txt", index + 1);
        rename(
            &repo,
            &path,
            &next_path,
            &format!("Rename {index}"),
            &format!("2020-01-{:02}T00:00:00Z", index + 2),
        );
        path = next_path;
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
    assert!(text.contains("unknown"), "{text}");
    assert!(text.contains("1 event(s) recorded"), "{text}");
    assert!(text.contains(&start), "{text}");
}

#[test]
fn fate_symbol_tracks_local_edits_and_stops_on_whole_body_rewrite() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "lib.rs",
        "fn alpha() {\n    let a = 1;\n    let keep = 0;\n}\nfn beta() {}\n",
        "Create",
        "2020-01-01T00:00:00Z",
    );
    // Local edit inside the body: tracked as a modification, tracing continues.
    let edit = commit(
        &repo,
        "lib.rs",
        "fn alpha() {\n    let a = 2;\n    let keep = 0;\n}\nfn beta() {}\n",
        "Adjust alpha",
        "2020-01-02T00:00:00Z",
    );
    let insertion = commit(
        &repo,
        "lib.rs",
        "fn alpha() {\n    let a = 2;\n    let b = 3;\n    let keep = 0;\n}\nfn beta() {}\n",
        "Insert in alpha",
        "2020-01-03T00:00:00Z",
    );
    repo.index();
    let report = json(
        &repo,
        &[
            "fate", "lib.rs", "--symbol", "alpha", "--at", &start, "--json",
        ],
    );
    assert_eq!(report["final_state"], "reached_endpoint", "{report}");
    assert_eq!(report["symbol_selection"]["qualified_name"], "alpha");
    assert_eq!(report["symbol_selection"]["kind"], "function");
    assert_eq!(report["symbol_selection"]["start_line"], 1);
    assert_eq!(report["symbol_selection"]["end_line"], 4);
    let events = report["events"].as_array().unwrap();
    assert_eq!(events.len(), 2, "{report}");
    assert_eq!(events[0]["commit_id"], edit);
    assert_eq!(events[1]["commit_id"], insertion);
    assert_eq!(events[1]["relationship"], "modified");
    assert_eq!(events[0]["relationship"], "modified");
    assert_eq!(events[0]["before"]["start_line"], 1);
    assert_eq!(events[0]["after"]["start_line"], 1);

    // Whole-body rewrite keeping the name must stop as unknown.
    let start = commit(
        &repo,
        "lib.rs",
        "fn alpha() {\n    let a = 1;\n}\nfn beta() {}\n",
        "Create",
        "2020-01-01T00:00:00Z",
    );
    let rewrite = commit(
        &repo,
        "lib.rs",
        "fn alpha() {\n    let b = 10;\n}\nfn beta() {}\n",
        "Rewrite alpha body",
        "2020-01-02T00:00:00Z",
    );
    repo.index();
    let report = json(
        &repo,
        &[
            "fate", "lib.rs", "--symbol", "alpha", "--at", &start, "--json",
        ],
    );
    assert_eq!(report["final_state"], "unknown", "{report}");
    assert_eq!(report["stopped_at"], rewrite);
    assert_eq!(report["stop_reason"]["code"], "symbol_body_rewritten");
    assert_eq!(report["last_location"]["path"], "lib.rs");
    assert_eq!(report["associations"][0]["path"], "lib.rs");
    assert_eq!(report["associations"][0]["line"], 1);
}

#[test]
fn fate_symbol_split_reports_an_unverified_association() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "lib.rs",
        "fn tracked(value: i32) -> i32 {\n    let result = value + 1;\n    result\n}\n",
        "Create tracked function",
        "2020-01-01T00:00:00Z",
    );
    let split = commit(
        &repo,
        "lib.rs",
        "fn tracked(value: i32) -> i32 {\n    let base = value;\n    let result = base + 1;\n    result\n}\n",
        "Split tracked expression",
        "2020-01-02T00:00:00Z",
    );
    repo.index();
    let report = json(
        &repo,
        &[
            "fate", "lib.rs", "--symbol", "tracked", "--at", &start, "--json",
        ],
    );
    assert_eq!(report["final_state"], "unknown", "{report}");
    assert_eq!(report["stopped_at"], split);
    assert_eq!(report["associations"][0]["path"], "lib.rs", "{report}");
    assert_eq!(report["associations"][0]["line"], 1, "{report}");
}

#[test]
fn fate_symbol_rejects_missing_and_ambiguous_and_line_overlap() {
    let repo = TestRepo::new();
    commit(
        &repo,
        "lib.rs",
        "mod net {\n    fn parse() {}\n    fn parse_v2() {}\n}\nfn parse() {}\n",
        "Create",
        "2020-01-01T00:00:00Z",
    );
    repo.index();

    let missing = repo.run([
        "fate", "lib.rs", "--symbol", "absent", "--at", "HEAD", "--json",
    ]);
    assert_eq!(missing.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&missing.stderr);
    assert!(stderr.contains("no supported declaration"), "{stderr}");

    let ambiguous = repo.run(["fate", "lib.rs", "--symbol", "parse", "--at", "HEAD"]);
    assert_eq!(ambiguous.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&ambiguous.stderr);
    assert!(stderr.contains("ambiguous"), "{stderr}");
    assert!(stderr.contains("net::parse"), "{stderr}");

    let exclusive = repo.run([
        "fate", "lib.rs", "--symbol", "parse", "--line", "1", "--at", "HEAD",
    ]);
    assert_ne!(exclusive.status.code(), Some(0));
}

#[test]
fn fate_symbol_survives_renames_and_coordinate_shifts() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "old.rs",
        "fn tracked() {\n    let a = 1;\n}\nfn other() {}\n",
        "Create",
        "2020-01-01T00:00:00Z",
    );
    rename(&repo, "old.rs", "new.rs", "Rename", "2020-01-02T00:00:00Z");
    // Unrelated insertion before the symbol shifts its start line.
    let edit = commit(
        &repo,
        "new.rs",
        "fn first() {}\n\nfn tracked() {\n    let a = 1;\n}\nfn other() {}\n",
        "Insert before",
        "2020-01-03T00:00:00Z",
    );
    repo.index();
    let report = json(
        &repo,
        &[
            "fate", "old.rs", "--symbol", "tracked", "--at", &start, "--json",
        ],
    );
    assert_eq!(report["final_state"], "reached_endpoint", "{report}");
    let events = report["events"].as_array().unwrap();
    assert_eq!(events.len(), 2, "{report}");
    assert_eq!(events[0]["relationship"], "rename");
    assert_eq!(events[0]["after"]["path"], "new.rs");
    assert_eq!(events[1]["relationship"], "shifted");
    assert_eq!(events[1]["commit_id"], edit);
    assert_eq!(events[1]["after"]["start_line"], 3);
    let location = &report["last_location"];
    assert_eq!(location["path"], "new.rs");
    assert_eq!(location["start_line"], 3);
}

#[test]
fn fate_symbol_stops_on_declaration_replacement() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "lib.rs",
        "fn tracked() {\n    let a = 1;\n}\n",
        "Create",
        "2020-01-01T00:00:00Z",
    );
    // The declaration itself is replaced: same lines, different name.
    let replacement = commit(
        &repo,
        "lib.rs",
        "fn renamed() {\n    let a = 1;\n}\n",
        "Replace declaration",
        "2020-01-02T00:00:00Z",
    );
    repo.index();
    let report = json(
        &repo,
        &[
            "fate", "lib.rs", "--symbol", "tracked", "--at", &start, "--json",
        ],
    );
    assert_eq!(report["final_state"], "unknown", "{report}");
    assert_eq!(report["stopped_at"], replacement);
    assert_eq!(report["stop_reason"]["code"], "symbol_declaration_replaced");
}

#[test]
fn fate_symbol_comment_only_survival_is_not_a_modification() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "lib.rs",
        "fn tracked() {\n    let a = 1;\n}\n",
        "Create",
        "2020-01-01T00:00:00Z",
    );
    // Comment and whitespace changes inside the body do not modify the code.
    let _comment = commit(
        &repo,
        "lib.rs",
        "fn tracked() {\n    // note\n    let a = 1;\n}\n",
        "Add comment",
        "2020-01-02T00:00:00Z",
    );
    repo.index();
    let report = json(
        &repo,
        &[
            "fate", "lib.rs", "--symbol", "tracked", "--at", &start, "--json",
        ],
    );
    assert_eq!(report["final_state"], "reached_endpoint", "{report}");
    assert_eq!(report["events"].as_array().unwrap().len(), 0, "{report}");
}

#[test]
fn fate_symbol_stops_when_only_comments_and_delimiters_survive() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "lib.rs",
        "fn tracked() {\n    let a = 1;\n}\n",
        "Create",
        "2020-01-01T00:00:00Z",
    );
    let rewrite = commit(
        &repo,
        "lib.rs",
        "fn tracked() {\n    // replaced body\n}\n",
        "Replace body with comment",
        "2020-01-02T00:00:00Z",
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "fate", "lib.rs", "--symbol", "tracked", "--at", &start, "--json",
        ],
    );
    assert_eq!(report["final_state"], "unknown", "{report}");
    assert_eq!(report["stopped_at"], rewrite);
    assert_eq!(report["stop_reason"]["code"], "symbol_body_rewritten");
}

#[test]
fn fate_symbol_stops_when_body_boundaries_cannot_map_uniquely() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "lib.rs",
        "fn tracked() {\n    let a = 1;\n}\n",
        "Create",
        "2020-01-01T00:00:00Z",
    );
    let rewrite = commit(
        &repo,
        "lib.rs",
        "fn tracked() {\n    let b = 2;\n    let c = 3;\n}\n",
        "Expand body rewrite",
        "2020-01-02T00:00:00Z",
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "fate", "lib.rs", "--symbol", "tracked", "--at", &start, "--json",
        ],
    );
    assert_eq!(report["final_state"], "unknown", "{report}");
    assert_eq!(report["stopped_at"], rewrite);
    assert_eq!(
        report["stop_reason"]["code"],
        "unreliable_boundary_correspondence"
    );
}

#[test]
fn fate_symbol_does_not_restart_after_declaration_reappears() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "lib.rs",
        "fn tracked() {\n    let a = 1;\n}\n",
        "Create",
        "2020-01-01T00:00:00Z",
    );
    let replacement = commit(
        &repo,
        "lib.rs",
        "fn renamed() {\n    let a = 1;\n}\n",
        "Rename declaration",
        "2020-01-02T00:00:00Z",
    );
    commit(
        &repo,
        "lib.rs",
        "fn tracked() {\n    let a = 1;\n}\n",
        "Reintroduce old declaration",
        "2020-01-03T00:00:00Z",
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "fate", "lib.rs", "--symbol", "tracked", "--at", &start, "--json",
        ],
    );
    assert_eq!(report["final_state"], "unknown", "{report}");
    assert_eq!(report["stopped_at"], replacement);
    assert_eq!(report["stop_reason"]["code"], "symbol_declaration_replaced");
    assert!(report["events"].as_array().unwrap().is_empty(), "{report}");
}

#[test]
fn fate_symbol_renders_text_output() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "lib.rs",
        "fn tracked() {\n    let a = 1;\n    let b = 2;\n}\n",
        "Create",
        "2020-01-01T00:00:00Z",
    );
    commit(
        &repo,
        "lib.rs",
        "fn tracked() {\n    let a = 3;\n    let b = 2;\n}\n",
        "Modify body",
        "2020-01-02T00:00:00Z",
    );
    repo.index();

    let output = repo.run(["fate", "lib.rs", "--symbol", "tracked", "--at", &start]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
    assert!(text.contains("tracked"), "{text}");
    assert!(text.contains("modified"), "{text}");
}

#[test]
fn fate_symbol_records_signature_edits_with_stable_body() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "lib.rs",
        "fn tracked(value: i32) {\n    let a = 1;\n}\n",
        "Create",
        "2020-01-01T00:00:00Z",
    );
    let edit = commit(
        &repo,
        "lib.rs",
        "fn tracked(value: i64) {\n    let a = 1;\n}\n",
        "Change parameter type",
        "2020-01-02T00:00:00Z",
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "fate", "lib.rs", "--symbol", "tracked", "--at", &start, "--json",
        ],
    );
    assert_eq!(report["final_state"], "reached_endpoint", "{report}");
    let events = report["events"].as_array().unwrap();
    assert_eq!(events.len(), 1, "{report}");
    assert_eq!(events[0]["commit_id"], edit);
    assert_eq!(events[0]["relationship"], "modified");
}

#[test]
fn fate_symbol_does_not_count_python_comments_as_body_correspondence() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "tracked.py",
        "def tracked():\n    # note\n    value = 1\n",
        "Create",
        "2020-01-01T00:00:00Z",
    );
    let edit = commit(
        &repo,
        "tracked.py",
        "def tracked():\n    # note\n    value = 2\n",
        "Edit body, keep comment",
        "2020-01-02T00:00:00Z",
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "fate",
            "tracked.py",
            "--symbol",
            "tracked",
            "--at",
            &start,
            "--json",
        ],
    );
    assert_eq!(report["final_state"], "unknown", "{report}");
    assert_eq!(report["stopped_at"], edit);
    assert_eq!(report["stop_reason"]["code"], "symbol_body_rewritten");
    assert!(report["events"].as_array().unwrap().is_empty(), "{report}");
}

#[test]
fn fate_symbol_records_python_triple_quoted_string_edits() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "tracked.py",
        "def tracked():\n    text = \"\"\"\n    # old text\n    \"\"\"\n    value = 1\n",
        "Create",
        "2020-01-01T00:00:00Z",
    );
    let edit = commit(
        &repo,
        "tracked.py",
        "def tracked():\n    text = \"\"\"\n    # new text\n    \"\"\"\n    value = 1\n",
        "Edit triple-quoted string",
        "2020-01-02T00:00:00Z",
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "fate",
            "tracked.py",
            "--symbol",
            "tracked",
            "--at",
            &start,
            "--json",
        ],
    );
    assert_eq!(report["final_state"], "reached_endpoint", "{report}");
    let events = report["events"].as_array().unwrap();
    assert_eq!(events.len(), 1, "{report}");
    assert_eq!(events[0]["commit_id"], edit);
    assert_eq!(events[0]["relationship"], "modified");
}

#[test]
fn fate_symbol_follows_strict_cross_file_move_and_later_edits() {
    let repo = TestRepo::new();
    let original = "fn tracked(value: i32) -> i32 {\n    let base = value + 1;\n    let extra = 2;\n    base + extra\n}\n";
    let edited = "fn tracked(value: i32) -> i32 {\n    let base = value + 2;\n    let extra = 2;\n    base + extra\n}\n";
    let initial = format!("{original}fn remaining() {{}}\n");
    let start = commit(
        &repo,
        "src/lib.rs",
        &initial,
        "Create tracked function",
        "2020-01-01T00:00:00Z",
    );
    fs::write(repo.dir.path().join("src/lib.rs"), "fn remaining() {}\n")
        .expect("retain the original file without the moved target");
    let moved = commit(
        &repo,
        "src/tracked.rs",
        original,
        "Move tracked function",
        "2020-01-02T00:00:00Z",
    );
    let edited_at = commit(
        &repo,
        "src/tracked.rs",
        edited,
        "Edit tracked function",
        "2020-01-03T00:00:00Z",
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "fate",
            "src/lib.rs",
            "--symbol",
            "tracked",
            "--at",
            &start,
            "--json",
        ],
    );
    assert_eq!(report["final_state"], "reached_endpoint", "{report}");
    let events = report["events"].as_array().unwrap();
    assert_eq!(events.len(), 2, "{report}");
    assert_eq!(events[0]["commit_id"], moved);
    assert_eq!(events[0]["relationship"], "move");
    assert_eq!(events[0]["before"]["path"], "src/lib.rs");
    assert_eq!(events[0]["after"]["path"], "src/tracked.rs");
    assert_eq!(events[1]["commit_id"], edited_at);
    assert_eq!(events[1]["relationship"], "modified");
    assert_eq!(events[1]["before"]["path"], "src/tracked.rs");
    assert_eq!(report["last_location"]["path"], "src/tracked.rs");
}

#[test]
fn fate_symbol_follows_same_file_move_and_later_edits() {
    let repo = TestRepo::new();
    let tracked = "fn tracked(value: i32) -> i32 {\n    let base = value + 1;\n    let extra = 2;\n    base + extra\n}\n";
    let remaining = "fn remaining() -> i32 {\n    let a = 1;\n    let b = 2;\n    let c = 3;\n    let d = 4;\n    a + b + c + d\n}\n";
    let original = format!("{tracked}{remaining}");
    let moved_content = format!("{remaining}{tracked}");
    let edited = moved_content.replace("value + 1", "value + 2");
    let start = commit(
        &repo,
        "src/lib.rs",
        &original,
        "Create tracked function",
        "2020-01-01T00:00:00Z",
    );
    let moved = commit(
        &repo,
        "src/lib.rs",
        &moved_content,
        "Move tracked function within file",
        "2020-01-02T00:00:00Z",
    );
    let edited_at = commit(
        &repo,
        "src/lib.rs",
        &edited,
        "Edit tracked function",
        "2020-01-03T00:00:00Z",
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "fate",
            "src/lib.rs",
            "--symbol",
            "tracked",
            "--at",
            &start,
            "--json",
        ],
    );
    assert_eq!(report["final_state"], "reached_endpoint", "{report}");
    let events = report["events"].as_array().unwrap();
    assert_eq!(events.len(), 2, "{report}");
    assert_eq!(events[0]["commit_id"], moved);
    assert_eq!(events[0]["relationship"], "move");
    assert_eq!(events[0]["before"]["path"], "src/lib.rs");
    assert_eq!(events[0]["after"]["path"], "src/lib.rs");
    assert!(
        events[0]["after"]["start_line"].as_i64().unwrap()
            > events[0]["before"]["start_line"].as_i64().unwrap()
    );
    assert_eq!(events[1]["commit_id"], edited_at);
    assert_eq!(events[1]["relationship"], "modified");
}

#[test]
fn fate_line_follows_strict_cross_file_move() {
    let repo = TestRepo::new();
    let tracked = "fn tracked(value: i32) -> i32 {\n    let base = value + 1;\n    let extra = 2;\n    base + extra\n}\n";
    let remaining = "fn remaining() {}\n";
    let initial = format!("{tracked}{remaining}");
    let start = commit(
        &repo,
        "src/lib.rs",
        &initial,
        "Create tracked line",
        "2020-01-01T00:00:00Z",
    );
    fs::write(repo.dir.path().join("src/lib.rs"), remaining)
        .expect("retain old file without tracked function");
    let moved = commit(
        &repo,
        "src/tracked.rs",
        tracked,
        "Move tracked function",
        "2020-01-02T00:00:00Z",
    );
    let edited = tracked.replace("let extra = 2", "let extra = 3");
    commit(
        &repo,
        "src/tracked.rs",
        &edited,
        "Edit neighboring line",
        "2020-01-03T00:00:00Z",
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "fate",
            "src/lib.rs",
            "--line",
            "2",
            "--at",
            &start,
            "--json",
        ],
    );
    assert_eq!(report["final_state"], "reached_endpoint", "{report}");
    let events = report["events"].as_array().unwrap();
    assert_eq!(events.len(), 1, "{report}");
    assert_eq!(events[0]["commit_id"], moved);
    assert_eq!(events[0]["relationship"], "move");
    assert_eq!(events[0]["before"]["path"], "src/lib.rs");
    assert_eq!(events[0]["before"]["line"], 2);
    assert_eq!(events[0]["after"]["path"], "src/tracked.rs");
    assert_eq!(events[0]["after"]["line"], 2);
    assert_eq!(report["last_location"]["path"], "src/tracked.rs");
}

#[test]
fn fate_follows_cross_file_move_when_source_file_is_removed() {
    let repo = TestRepo::new();
    let tracked = "fn tracked(value: i32) -> i32 {\n    let base = value + 1;\n    let extra = 2;\n    base + extra\n}\n";
    let filler = format!(
        "fn old_helper() {{\n{} }}\n",
        "    let item = 1;\n".repeat(80)
    );
    let initial = format!("{tracked}{filler}");
    let start = commit(
        &repo,
        "src/lib.rs",
        &initial,
        "Create target and helper",
        "2020-01-01T00:00:00Z",
    );
    fs::remove_file(repo.dir.path().join("src/lib.rs")).expect("remove source file");
    let moved = commit(
        &repo,
        "src/tracked.rs",
        tracked,
        "Move target and remove source file",
        "2020-01-02T00:00:00Z",
    );
    repo.index();

    let line_report = json(
        &repo,
        &[
            "fate",
            "src/lib.rs",
            "--line",
            "2",
            "--at",
            &start,
            "--json",
        ],
    );
    assert_eq!(
        line_report["final_state"], "reached_endpoint",
        "{line_report}"
    );
    assert_eq!(
        line_report["events"][0]["commit_id"], moved,
        "{line_report}"
    );
    assert_eq!(
        line_report["events"][0]["relationship"], "move",
        "{line_report}"
    );
    assert_eq!(
        line_report["last_location"]["path"], "src/tracked.rs",
        "{line_report}"
    );

    let symbol_report = json(
        &repo,
        &[
            "fate",
            "src/lib.rs",
            "--symbol",
            "tracked",
            "--at",
            &start,
            "--json",
        ],
    );
    assert_eq!(
        symbol_report["final_state"], "reached_endpoint",
        "{symbol_report}"
    );
    assert_eq!(
        symbol_report["events"][0]["commit_id"], moved,
        "{symbol_report}"
    );
    assert_eq!(
        symbol_report["events"][0]["relationship"], "move",
        "{symbol_report}"
    );
    assert_eq!(
        symbol_report["last_location"]["path"], "src/tracked.rs",
        "{symbol_report}"
    );
}

#[test]
fn fate_reports_unknown_cross_file_moves_that_rewrite_the_target() {
    let repo = TestRepo::new();
    let tracked = "fn tracked(value: i32) -> i32 {\n    let base = value + 1;\n    let extra = 2;\n    base + extra\n}\n";
    let rewritten = tracked.replace("value + 1", "value + 2");
    let filler = format!(
        "fn old_helper() {{\n{} }}\n",
        "    let item = 1;\n".repeat(80)
    );
    let initial = format!("{tracked}{filler}");
    let start = commit(
        &repo,
        "src/lib.rs",
        &initial,
        "Create target and helper",
        "2020-01-01T00:00:00Z",
    );
    fs::remove_file(repo.dir.path().join("src/lib.rs")).expect("remove source file");
    commit(
        &repo,
        "src/tracked.rs",
        &rewritten,
        "Move and rewrite target",
        "2020-01-02T00:00:00Z",
    );
    repo.index();

    let line_report = json(
        &repo,
        &[
            "fate",
            "src/lib.rs",
            "--line",
            "2",
            "--at",
            &start,
            "--json",
        ],
    );
    assert_eq!(line_report["final_state"], "unknown", "{line_report}");
    assert_eq!(
        line_report["stop_reason"]["code"], "move_candidate_unverified",
        "{line_report}"
    );
    assert_eq!(line_report["associations"][0]["path"], "src/tracked.rs");
    assert_eq!(line_report["associations"][0]["line"], 2);

    let symbol_report = json(
        &repo,
        &[
            "fate",
            "src/lib.rs",
            "--symbol",
            "tracked",
            "--at",
            &start,
            "--json",
        ],
    );
    assert_eq!(symbol_report["final_state"], "unknown", "{symbol_report}");
    assert_eq!(
        symbol_report["stop_reason"]["code"], "move_candidate_unverified",
        "{symbol_report}"
    );
    assert_eq!(symbol_report["associations"][0]["path"], "src/tracked.rs");
}

#[test]
fn fate_does_not_follow_a_preexisting_line_match_in_a_changed_file() {
    let repo = TestRepo::new();
    fs::create_dir_all(repo.dir.path().join("src")).expect("create source directory");
    let tracked = "fn tracked(value: i32) -> i32 {\n    let base = value + 1;\n    let extra = 2;\n    base + extra\n}\n";
    let decoy = format!("{tracked}fn existing() {{}}\n");
    fs::write(repo.dir.path().join("src/decoy.rs"), &decoy).expect("write preexisting duplicate");
    let initial = format!("{tracked}fn remaining() {{}}\n");
    let start = commit(
        &repo,
        "src/lib.rs",
        &initial,
        "Create duplicate line",
        "2020-01-01T00:00:00Z",
    );
    fs::write(
        repo.dir.path().join("src/decoy.rs"),
        format!("{decoy}// unrelated edit\n"),
    )
    .expect("change duplicate's file without moving it");
    commit(
        &repo,
        "src/lib.rs",
        "fn remaining() {}\n",
        "Delete selected line and edit duplicate file",
        "2020-01-02T00:00:00Z",
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "fate",
            "src/lib.rs",
            "--line",
            "2",
            "--at",
            &start,
            "--json",
        ],
    );
    assert_eq!(report["final_state"], "deleted", "{report}");
    assert_eq!(report["last_location"]["path"], "src/lib.rs", "{report}");
    assert!(
        report["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|event| event["relationship"] != "move")
    );
}

#[test]
fn fate_does_not_follow_a_preexisting_symbol_match_in_a_changed_file() {
    let repo = TestRepo::new();
    fs::create_dir_all(repo.dir.path().join("src")).expect("create source directory");
    let tracked = "fn tracked(value: i32) -> i32 {\n    let base = value + 1;\n    let extra = 2;\n    base + extra\n}\n";
    let decoy = format!("{tracked}fn existing() {{}}\n");
    fs::write(repo.dir.path().join("src/decoy.rs"), &decoy).expect("write preexisting duplicate");
    let initial = format!("{tracked}fn remaining() {{}}\n");
    let start = commit(
        &repo,
        "src/lib.rs",
        &initial,
        "Create duplicate symbol",
        "2020-01-01T00:00:00Z",
    );
    fs::write(
        repo.dir.path().join("src/decoy.rs"),
        format!("{decoy}// unrelated edit\n"),
    )
    .expect("change duplicate's file without moving it");
    commit(
        &repo,
        "src/lib.rs",
        "fn remaining() {}\n",
        "Delete selected symbol and edit duplicate file",
        "2020-01-02T00:00:00Z",
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "fate",
            "src/lib.rs",
            "--symbol",
            "tracked",
            "--at",
            &start,
            "--json",
        ],
    );
    assert_ne!(report["final_state"], "reached_endpoint", "{report}");
    assert_eq!(report["last_location"]["path"], "src/lib.rs", "{report}");
    assert!(
        report["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|event| event["relationship"] != "move")
    );
}

#[test]
fn fate_does_not_choose_between_competing_line_moves() {
    let repo = TestRepo::new();
    let tracked = "fn tracked(value: i32) -> i32 {\n    let base = value + 1;\n    let extra = 2;\n    base + extra\n}\n";
    let initial = format!("{tracked}fn remaining() {{}}\n");
    let start = commit(
        &repo,
        "src/lib.rs",
        &initial,
        "Create tracked line",
        "2020-01-01T00:00:00Z",
    );
    fs::write(repo.dir.path().join("src/lib.rs"), "fn remaining() {}\n")
        .expect("retain the old file without the target");
    for index in 0..10 {
        let candidate_path = repo.dir.path().join(format!("src/candidate-{index:02}.rs"));
        fs::write(candidate_path, tracked).expect("write strict move candidate");
    }
    git(repo.dir.path(), ["add", "--all"]);
    let output = git_command(repo.dir.path())
        .args(["commit", "-m", "Move tracked line to competing files"])
        .env("GIT_AUTHOR_DATE", "2020-01-02T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-02T00:00:00Z")
        .output()
        .expect("run competing move commit");
    assert!(
        output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "fate",
            "src/lib.rs",
            "--line",
            "2",
            "--at",
            &start,
            "--json",
        ],
    );
    assert_eq!(report["final_state"], "unknown", "{report}");
    assert_eq!(
        report["stop_reason"]["code"], "ambiguous_move_candidates",
        "{report}"
    );
    let candidates = report["associations"].as_array().unwrap();
    assert_eq!(candidates.len(), 8, "{report}");
    assert_eq!(
        report["associations_truncated"].as_bool(),
        Some(true),
        "{report}"
    );
    assert_eq!(candidates[0]["path"], "src/candidate-00.rs", "{report}");
    assert_eq!(candidates[7]["path"], "src/candidate-07.rs", "{report}");
}

#[test]
fn fate_does_not_choose_between_competing_symbol_moves() {
    let repo = TestRepo::new();
    let tracked = "fn tracked(value: i32) -> i32 {\n    let base = value + 1;\n    let extra = 2;\n    base + extra\n}\n";
    let initial = format!("{tracked}fn remaining() {{}}\n");
    let start = commit(
        &repo,
        "src/lib.rs",
        &initial,
        "Create tracked function",
        "2020-01-01T00:00:00Z",
    );
    fs::write(repo.dir.path().join("src/lib.rs"), "fn remaining() {}\n")
        .expect("remove tracked function from its original file");
    fs::write(repo.dir.path().join("src/first.rs"), tracked)
        .expect("write first strict move candidate");
    fs::write(repo.dir.path().join("src/second.rs"), tracked)
        .expect("write second strict move candidate");
    git(repo.dir.path(), ["add", "--all"]);
    let output = git_command(repo.dir.path())
        .args(["commit", "-m", "Copy tracked function to competing files"])
        .env("GIT_AUTHOR_DATE", "2020-01-03T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-03T00:00:00Z")
        .output()
        .expect("run competing symbol move commit");
    assert!(
        output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "fate",
            "src/lib.rs",
            "--symbol",
            "tracked",
            "--at",
            &start,
            "--json",
        ],
    );
    assert_eq!(report["final_state"], "unknown", "{report}");
    assert_eq!(
        report["stop_reason"]["code"], "ambiguous_move_candidates",
        "{report}"
    );
    assert_eq!(report["last_location"]["path"], "src/lib.rs", "{report}");
    assert!(report["events"].as_array().unwrap().is_empty(), "{report}");
    let candidates = report["associations"].as_array().unwrap();
    assert_eq!(candidates.len(), 2, "{report}");
    assert!(
        candidates
            .iter()
            .any(|candidate| candidate["path"] == "src/first.rs")
    );
    assert!(
        candidates
            .iter()
            .any(|candidate| candidate["path"] == "src/second.rs")
    );
}
#[test]
fn fate_line_follows_strict_same_file_move() {
    let repo = TestRepo::new();
    let tracked = "fn tracked(value: i32) -> i32 {\n    let base = value + 1;\n    let extra = 2;\n    base + extra\n}\n";
    let remaining = "fn remaining() -> i32 {\n    let a = 1;\n    let b = 2;\n    let c = 3;\n    let d = 4;\n    a + b + c + d\n}\n";
    let original = format!("{tracked}{remaining}");
    let moved_content = format!("{remaining}{tracked}");
    let edited = moved_content.replace("let extra = 2", "let extra = 3");
    let start = commit(
        &repo,
        "src/lib.rs",
        &original,
        "Create tracked line",
        "2020-01-01T00:00:00Z",
    );
    let moved = commit(
        &repo,
        "src/lib.rs",
        &moved_content,
        "Move tracked function within file",
        "2020-01-02T00:00:00Z",
    );
    commit(
        &repo,
        "src/lib.rs",
        &edited,
        "Edit neighboring line",
        "2020-01-03T00:00:00Z",
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "fate",
            "src/lib.rs",
            "--line",
            "2",
            "--at",
            &start,
            "--json",
        ],
    );
    assert_eq!(report["final_state"], "reached_endpoint", "{report}");
    let events = report["events"].as_array().unwrap();
    assert_eq!(events.len(), 1, "{report}");
    assert_eq!(events[0]["commit_id"], moved);
    assert_eq!(events[0]["relationship"], "move");
    assert_eq!(events[0]["before"]["path"], "src/lib.rs");
    assert_eq!(events[0]["after"]["path"], "src/lib.rs");
    assert!(
        events[0]["after"]["line"].as_i64().unwrap()
            > events[0]["before"]["line"].as_i64().unwrap()
    );
}

#[test]
fn fate_symbol_follows_indented_cross_file_move() {
    let repo = TestRepo::new();
    let original = "fn tracked(value: i32) -> i32 {\n    let base = value + 1;\n    let extra = 2;\n    base + extra\n}\n";
    let initial = format!("{original}fn remaining() {{}}\n");
    let indented = original
        .lines()
        .map(|line| format!("    {line}\r\n"))
        .collect::<String>();
    let later = format!("// moved again\r\n{indented}");
    let start = commit(
        &repo,
        "src/lib.rs",
        &initial,
        "Create tracked function",
        "2020-01-01T00:00:00Z",
    );
    fs::write(repo.dir.path().join("src/lib.rs"), "fn remaining() {}\n")
        .expect("retain the original file without the target");
    let moved = commit(
        &repo,
        "src/tracked.rs",
        &indented,
        "Move and indent tracked function",
        "2020-01-02T00:00:00Z",
    );
    let shifted = commit(
        &repo,
        "src/tracked.rs",
        &later,
        "Add comment before moved function",
        "2020-01-03T00:00:00Z",
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "fate",
            "src/lib.rs",
            "--symbol",
            "tracked",
            "--at",
            &start,
            "--json",
        ],
    );
    assert_eq!(report["final_state"], "reached_endpoint", "{report}");
    let events = report["events"].as_array().unwrap();
    assert_eq!(events.len(), 2, "{report}");
    assert_eq!(events[0]["commit_id"], moved);
    assert_eq!(events[0]["relationship"], "move");
    assert_eq!(events[0]["after"]["path"], "src/tracked.rs");
    assert_eq!(events[1]["commit_id"], shifted);
    assert_eq!(events[1]["relationship"], "shifted");
}

#[test]
fn fate_does_not_follow_move_that_changes_multiline_literal() {
    let repo = TestRepo::new();
    let original =
        "fn tracked() -> &'static str {\n    r#\"first\n    literal content\n    last\"#\n}\n";
    let changed = original.replace("literal content", "different content");
    let initial = format!("{original}fn remaining() {{}}\n");
    let start = commit(
        &repo,
        "src/lib.rs",
        &initial,
        "Create tracked function",
        "2020-01-01T00:00:00Z",
    );
    fs::write(repo.dir.path().join("src/lib.rs"), "fn remaining() {}\n")
        .expect("retain the original file without the target");
    commit(
        &repo,
        "src/tracked.rs",
        &changed,
        "Move and change literal content",
        "2020-01-02T00:00:00Z",
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "fate",
            "src/lib.rs",
            "--symbol",
            "tracked",
            "--at",
            &start,
            "--json",
        ],
    );
    assert_eq!(report["final_state"], "unknown", "{report}");
    assert!(report["events"].as_array().unwrap().is_empty(), "{report}");
}

#[test]
fn fate_patch_selects_target_relevant_hunks_amid_unrelated_edits() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "alpha\nbeta\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\n",
        "Add",
        "2020-01-01T00:00:00Z",
    );
    // The rewrite commit rewrites the tracked line (line 2) and an unrelated
    // distant line (line 12, beyond the 3-line git context window) plus a
    // sibling file; only the hunk touching the tracked line may be attached.
    commit(
        &repo,
        "other.txt",
        "seed\n",
        "Seed unrelated file",
        "2020-01-01T00:00:01Z",
    );
    fs::write(
        repo.dir.path().join("a.txt"),
        "alpha\nBETA\nc\nd\ne\nf\ng\nh\ni\nj\nk\nUNRELATED\n",
    )
    .expect("rewrite tracked file");
    fs::write(repo.dir.path().join("other.txt"), "noise\n").expect("edit unrelated file");
    git(repo.dir.path(), ["add", "--all"]);
    let rewrite_output = git_command(repo.dir.path())
        .args(["commit", "-m", "Rewrite beta with distant noise"])
        .env("GIT_AUTHOR_DATE", "2020-01-02T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-02T00:00:00Z")
        .output()
        .expect("run git rewrite commit");
    assert!(
        rewrite_output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&rewrite_output.stderr)
    );
    let rewrite = repo.head();
    repo.index();

    let report = json(
        &repo,
        &[
            "fate", "a.txt", "--line", "2", "--at", &start, "--patch", "--json",
        ],
    );
    assert_eq!(report["final_state"], "unknown");
    let events = report["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    let patch = &events[0]["patch"];
    assert_eq!(patch["status"], "available");
    assert_eq!(patch["commit_oid"], rewrite);
    let hunks = patch["hunks"].as_array().unwrap();
    assert_eq!(hunks.len(), 1, "{report}");
    let text = hunks[0]["text"].as_str().unwrap();
    assert!(text.contains("-beta"), "{text}");
    assert!(text.contains("+BETA"), "{text}");
    // Unrelated same-file (distant) and sibling-file edits stay out of the
    // attached hunk text.
    assert!(!text.contains("UNRELATED"), "{text}");
    assert!(!text.contains("noise"), "{text}");
}

#[test]
fn fate_patch_conclusions_match_without_patch() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "alpha\nbeta\ngamma\n",
        "Add",
        "2020-01-01T00:00:00Z",
    );
    let removal = commit(
        &repo,
        "a.txt",
        "alpha\n",
        "Delete lines",
        "2020-01-02T00:00:00Z",
    );
    repo.index();

    let plain = json(
        &repo,
        &["fate", "a.txt", "--line", "2", "--at", &start, "--json"],
    );
    let patched = json(
        &repo,
        &[
            "fate", "a.txt", "--line", "2", "--at", &start, "--patch", "--json",
        ],
    );
    // Enabling patches must not change the tracing conclusion.
    for field in [
        "final_state",
        "stopped_at",
        "total_events",
        "display_truncated",
    ] {
        assert_eq!(patched[field], plain[field], "{field}");
    }
    assert_eq!(patched["schema_version"], 3);
    assert_eq!(plain["schema_version"], 3);
    let patched_events = patched["events"].as_array().unwrap();
    let plain_events = plain["events"].as_array().unwrap();
    assert_eq!(patched_events.len(), plain_events.len());
    for (patched_event, plain_event) in patched_events.iter().zip(plain_events.iter()) {
        assert_eq!(patched_event["commit_id"], plain_event["commit_id"]);
        assert_eq!(patched_event["relationship"], plain_event["relationship"]);
        assert_eq!(patched_event["before"], plain_event["before"]);
        assert_eq!(patched_event["after"], plain_event["after"]);
    }
    assert_eq!(patched["stopped_at"], removal);
    assert_eq!(patched["final_state"], "deleted");
    let patch = &patched_events[0]["patch"];
    assert_eq!(patch["status"], "available");
    assert!(
        patch["hunks"][0]["text"]
            .as_str()
            .unwrap()
            .contains("-beta")
    );
}

#[test]
fn fate_patch_excerpts_are_bounded_and_disclose_truncation() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "start\nbeta\nend\n",
        "Add",
        "2020-01-01T00:00:00Z",
    );
    // One commit rewrites the tracked line inside a hunk that maps fine
    // (under the 16 KiB mapping budget) but exceeds the 8 KiB display
    // excerpt budget; the excerpt must be clipped with truncation disclosed.
    let mut contents = String::from("start\nBETA\nend\n");
    for index in 0..300 {
        contents.push_str(&format!("filler {index} to grow the display excerpt\n"));
    }
    let rewrite = commit(
        &repo,
        "a.txt",
        &contents,
        "Rewrite beta inside large hunk",
        "2020-01-02T00:00:00Z",
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "fate", "a.txt", "--line", "2", "--at", &start, "--patch", "--json",
        ],
    );
    assert_eq!(report["final_state"], "unknown");
    assert_eq!(report["stopped_at"], rewrite);
    let events = report["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    let patch = &events[0]["patch"];
    assert_eq!(patch["status"], "available");
    assert_eq!(patch["truncated"], true);
    let text = patch["hunks"][0]["text"].as_str().unwrap();
    assert!(text.contains("-beta"), "{text}");
    assert!(text.len() <= 8192, "excerpt {} bytes", text.len());
    let text_output = repo.run(["fate", "a.txt", "--line", "2", "--at", &start, "--patch"]);
    let rendered = String::from_utf8_lossy(&text_output.stdout);
    assert!(rendered.contains("-beta"), "{rendered}");
    assert!(rendered.contains("truncated"), "{rendered}");
}

#[test]
fn fate_patch_unavailable_material_stops_without_substitution() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "start\nbeta\nend\n",
        "Add",
        "2020-01-01T00:00:00Z",
    );
    // The tracked line is rewritten inside a hunk exceeding the material scan
    // budget (MAX_HUNK_BYTES = 16 KiB): mapping material is unavailable, so
    // tracking must stop as unknown instead of guessing or substituting hunks.
    let mut contents = String::from("start\nBETA\nend\n");
    for index in 0..3000 {
        contents.push_str(&format!(
            "filler line {index} to exceed the mapping budget\n"
        ));
    }
    let rewrite = commit(
        &repo,
        "a.txt",
        &contents,
        "Rewrite beta beyond material budget",
        "2020-01-02T00:00:00Z",
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "fate", "a.txt", "--line", "2", "--at", &start, "--patch", "--json",
        ],
    );
    assert_eq!(report["final_state"], "unknown");
    assert_eq!(report["stopped_at"], rewrite);
    assert_eq!(report["stop_reason"]["code"], "missing_patch_material");
}

#[test]
fn fate_patch_rename_event_discloses_missing_hunk_material() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "old.txt",
        "alpha\nbeta\n",
        "Add",
        "2020-01-01T00:00:00Z",
    );
    rename(
        &repo,
        "old.txt",
        "new.txt",
        "Rename",
        "2020-01-02T00:00:00Z",
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "fate", "old.txt", "--line", "2", "--at", &start, "--patch", "--json",
        ],
    );
    assert_eq!(report["final_state"], "reached_endpoint");
    let events = report["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["relationship"], "rename");
    // A pure rename has no text hunk; the excerpt must disclose that rather
    // than substitute unrelated material.
    let patch = &events[0]["patch"];
    assert_eq!(patch["status"], "unavailable");
    assert_eq!(patch["hunks"], serde_json::json!([]));
    let text_output = repo.run(["fate", "old.txt", "--line", "2", "--at", &start, "--patch"]);
    let rendered = String::from_utf8_lossy(&text_output.stdout);
    assert!(rendered.contains("unavailable"), "{rendered}");
}

#[test]
fn fate_tracks_a_side_branch_anchor_through_the_merge_result() {
    let repo = TestRepo::new();
    commit(
        &repo,
        "a.txt",
        "alpha\nbeta\n",
        "Base",
        "2020-01-01T00:00:00Z",
    );
    git(repo.dir.path(), ["switch", "-c", "side"]);
    let start = commit(
        &repo,
        "a.txt",
        "prefix\nalpha\nbeta\n",
        "Add target on side branch",
        "2020-01-02T00:00:00Z",
    );

    git(repo.dir.path(), ["switch", "-"]);
    commit(
        &repo,
        "other.txt",
        "unrelated\n",
        "Main-line change",
        "2020-01-03T00:00:00Z",
    );
    let merge = git_command(repo.dir.path())
        .args(["merge", "--no-ff", "--no-edit", "side"])
        .env("GIT_AUTHOR_DATE", "2020-01-04T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-04T00:00:00Z")
        .output()
        .expect("run git merge");
    assert!(
        merge.status.success(),
        "git merge failed: {}",
        String::from_utf8_lossy(&merge.stderr)
    );
    let endpoint = repo.head();
    repo.index();

    let report = json(
        &repo,
        &[
            "fate", "a.txt", "--line", "3", "--at", &start, "--to-rev", &endpoint, "--json",
        ],
    );

    assert_eq!(report["endpoint"], endpoint);
    assert_eq!(report["final_state"], "reached_endpoint");
    assert_eq!(report["last_location"]["path"], "a.txt");
    assert_eq!(report["last_location"]["line"], 3);
    assert_eq!(report["stopped_at"], Value::Null);
    let merge_event = report["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["commit_id"] == endpoint && event["relationship"] == "merge")
        .expect("merge event");
    assert_eq!(merge_event["predecessors"], serde_json::json!([start]));
    assert_eq!(
        merge_event["incoming"][0],
        serde_json::json!({"path": "a.txt", "line": 3})
    );
}

#[test]
fn fate_tracks_a_strict_line_move_through_a_merge() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "top\nalpha\nbeta\nbottom\n",
        "Base",
        "2020-01-01T00:00:00Z",
    );
    git(repo.dir.path(), ["switch", "-c", "side"]);
    let side_move = rename(
        &repo,
        "a.txt",
        "b.txt",
        "Move tracked file",
        "2020-01-02T00:00:00Z",
    );
    git(repo.dir.path(), ["switch", "-"]);
    commit(
        &repo,
        "other.txt",
        "unrelated\n",
        "Main-line change",
        "2020-01-03T00:00:00Z",
    );
    let merge = git_command(repo.dir.path())
        .args(["merge", "--no-ff", "--no-edit", "side"])
        .env("GIT_AUTHOR_DATE", "2020-01-04T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-04T00:00:00Z")
        .output()
        .expect("merge line move");
    assert!(
        merge.status.success(),
        "{}",
        String::from_utf8_lossy(&merge.stderr)
    );
    let endpoint = repo.head();
    repo.index();

    let report = json(
        &repo,
        &[
            "fate", "a.txt", "--line", "3", "--at", &start, "--to-rev", &endpoint, "--json",
        ],
    );

    assert_eq!(report["final_state"], "reached_endpoint", "{report:#}");
    assert_eq!(report["last_location"]["path"], "b.txt");
    assert_eq!(report["last_location"]["line"], 3);
    let merge_event = report["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["commit_id"] == endpoint)
        .expect("merge event");
    assert_eq!(merge_event["relationship"], "merge");
    assert_eq!(
        merge_event["predecessors"],
        serde_json::json!([start, side_move])
    );
    assert_eq!(merge_event["incoming"].as_array().unwrap().len(), 2);
}

#[test]
fn fate_reconciles_each_parent_coordinate_against_the_merge_result() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "top\nalpha\nbeta\nbottom\n",
        "Base",
        "2020-01-01T00:00:00Z",
    );
    git(repo.dir.path(), ["switch", "-c", "side"]);
    commit(
        &repo,
        "a.txt",
        "top\nside\nalpha\nbeta\nbottom\n",
        "Insert before target",
        "2020-01-02T00:00:00Z",
    );
    git(repo.dir.path(), ["switch", "-"]);
    commit(
        &repo,
        "a.txt",
        "top\nalpha\nbeta\nmain\nbottom\n",
        "Insert after target",
        "2020-01-03T00:00:00Z",
    );
    let merge = git_command(repo.dir.path())
        .args(["merge", "--no-ff", "--no-edit", "side"])
        .env("GIT_AUTHOR_DATE", "2020-01-04T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-04T00:00:00Z")
        .output()
        .expect("merge branch edits");
    assert!(
        merge.status.success(),
        "{}",
        String::from_utf8_lossy(&merge.stderr)
    );
    let endpoint = repo.head();
    repo.index();

    let report = json(
        &repo,
        &[
            "fate", "a.txt", "--line", "3", "--at", &start, "--to-rev", &endpoint, "--json",
        ],
    );

    assert_eq!(report["final_state"], "reached_endpoint", "{report:#}");
    assert_eq!(report["last_location"]["line"], 4);
    let merge_event = report["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["commit_id"] == endpoint)
        .expect("merge event");
    let incoming = merge_event["incoming"].as_array().unwrap();
    assert_eq!(incoming.len(), 2);
    let mut incoming_lines: Vec<_> = incoming
        .iter()
        .map(|location| {
            assert_eq!(location["path"], "a.txt");
            location["line"].as_u64().expect("incoming line")
        })
        .collect();
    incoming_lines.sort_unstable();
    assert_eq!(incoming_lines, [3, 4]);
}

#[test]
fn fate_merge_events_include_the_requested_first_parent_patch() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "alpha\nbeta\n",
        "Base",
        "2020-01-01T00:00:00Z",
    );
    git(repo.dir.path(), ["switch", "-c", "changed"]);
    commit(
        &repo,
        "a.txt",
        "alpha\nbeta changed\n",
        "Change target",
        "2020-01-02T00:00:00Z",
    );
    git(repo.dir.path(), ["switch", "-"]);
    commit(
        &repo,
        "other.txt",
        "unrelated\n",
        "Unrelated first-parent change",
        "2020-01-03T00:00:00Z",
    );
    let merge = git_command(repo.dir.path())
        .args(["merge", "--no-ff", "--no-edit", "changed"])
        .env("GIT_AUTHOR_DATE", "2020-01-04T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-04T00:00:00Z")
        .output()
        .expect("merge changed target");
    assert!(
        merge.status.success(),
        "{}",
        String::from_utf8_lossy(&merge.stderr)
    );
    let endpoint = repo.head();
    repo.index();

    let report = json(
        &repo,
        &[
            "fate", "a.txt", "--line", "2", "--at", &start, "--to-rev", &endpoint, "--patch",
            "--json",
        ],
    );
    let event = report["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["commit_id"] == endpoint)
        .expect("merge event");
    assert_eq!(event["relationship"], "merge");
    assert_eq!(event["patch"]["commit_oid"], endpoint);
    assert_eq!(event["patch"]["status"], "available", "{report:#}");
}

#[test]
fn fate_stops_when_relevant_merge_parents_continue_to_different_paths() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "alpha\nbeta\n",
        "Base",
        "2020-01-01T00:00:00Z",
    );
    git(repo.dir.path(), ["switch", "-c", "side"]);
    let side_rename = rename(
        &repo,
        "a.txt",
        "side.txt",
        "Side rename",
        "2020-01-10T00:00:00Z",
    );
    git(repo.dir.path(), ["switch", "-"]);
    let main_rename = rename(
        &repo,
        "a.txt",
        "main.txt",
        "Main rename",
        "2020-01-02T00:00:00Z",
    );
    let merge = git_command(repo.dir.path())
        .args(["merge", "--no-ff", "--no-edit", "side"])
        .env("GIT_AUTHOR_DATE", "2020-01-03T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-03T00:00:00Z")
        .output()
        .expect("run git merge");
    assert!(
        !merge.status.success(),
        "rename/rename merge should require a resolution"
    );
    fs::write(repo.dir.path().join("main.txt"), "alpha\nbeta\n").expect("restore main target path");
    fs::write(repo.dir.path().join("side.txt"), "alpha\nbeta\n").expect("restore side target path");
    let endpoint = commit(
        &repo,
        "main.txt",
        "alpha\nbeta\n",
        "Resolve both paths",
        "2020-01-04T00:00:00Z",
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "fate", "a.txt", "--line", "2", "--at", &start, "--to-rev", &endpoint, "--json",
        ],
    );

    assert_eq!(report["final_state"], "unknown");
    assert_eq!(report["stop_reason"]["code"], "merge_lineage_ambiguous");
    let merge_event = report["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["commit_id"] == endpoint)
        .expect("merge event");
    assert_eq!(merge_event["incoming"].as_array().unwrap().len(), 2);
    assert_eq!(merge_event["result_state"], "unknown");
    let text = repo.run([
        "fate", "a.txt", "--line", "2", "--at", &start, "--to-rev", &endpoint,
    ]);
    assert!(text.status.success());
    assert!(String::from_utf8_lossy(&text.stdout).contains("-> unknown"));
    let events = report["events"].as_array().unwrap();
    let side_position = events
        .iter()
        .position(|event| event["commit_id"] == side_rename)
        .expect("side rename event");
    let main_position = events
        .iter()
        .position(|event| event["commit_id"] == main_rename)
        .expect("main rename event");
    let merge_position = events
        .iter()
        .position(|event| event["commit_id"] == endpoint)
        .expect("merge event");
    assert!(side_position < merge_position);
    assert!(main_position < merge_position);
}

#[test]
fn fate_keeps_branch_deletion_when_another_parent_retains_the_target() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "alpha\nbeta\n",
        "Base",
        "2020-01-01T00:00:00Z",
    );
    git(repo.dir.path(), ["switch", "-c", "side"]);
    commit(
        &repo,
        "a.txt",
        "alpha\nbeta\nside\n",
        "Retain target",
        "2020-01-02T00:00:00Z",
    );
    git(repo.dir.path(), ["switch", "-"]);
    let deletion = delete(&repo, "a.txt", "Delete target", "2020-01-03T00:00:00Z");
    let merge = git_command(repo.dir.path())
        .args(["merge", "--no-ff", "--no-edit", "side"])
        .env("GIT_AUTHOR_DATE", "2020-01-04T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-04T00:00:00Z")
        .output()
        .expect("run git merge");
    assert!(
        !merge.status.success(),
        "delete/modify merge should require a resolution"
    );
    let endpoint = commit(
        &repo,
        "a.txt",
        "alpha\nbeta\nside\n",
        "Resolve by retaining target",
        "2020-01-05T00:00:00Z",
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "fate", "a.txt", "--line", "2", "--at", &start, "--to-rev", &endpoint, "--json",
        ],
    );

    assert_eq!(report["final_state"], "reached_endpoint");
    assert_eq!(report["last_location"]["line"], 2);
    let events = report["events"].as_array().unwrap();
    assert!(
        events
            .iter()
            .any(|event| { event["commit_id"] == deletion && event["relationship"] == "removed" })
    );
    assert!(
        events
            .iter()
            .any(|event| { event["commit_id"] == endpoint && event["relationship"] == "merge" })
    );
}

#[test]
fn fate_does_not_heal_unknown_branch_state_at_merge() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "alpha\nbeta\n",
        "Base",
        "2020-01-01T00:00:00Z",
    );
    git(repo.dir.path(), ["switch", "-c", "side"]);
    let rewrite = commit(
        &repo,
        "a.txt",
        "alpha\nBETA\n",
        "Rewrite target",
        "2020-01-02T00:00:00Z",
    );
    git(repo.dir.path(), ["switch", "-"]);
    commit(
        &repo,
        "other.txt",
        "unrelated\n",
        "Main change",
        "2020-01-03T00:00:00Z",
    );
    let merge = git_command(repo.dir.path())
        .args(["merge", "--no-ff", "--no-edit", "side"])
        .env("GIT_AUTHOR_DATE", "2020-01-04T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-04T00:00:00Z")
        .output()
        .expect("run git merge");
    assert!(
        merge.status.success(),
        "git merge failed: {}",
        String::from_utf8_lossy(&merge.stderr)
    );
    commit(
        &repo,
        "after.txt",
        "after merge\n",
        "After unknown merge",
        "2020-01-05T00:00:00Z",
    );
    git(repo.dir.path(), ["switch", "-c", "rescue", &start]);
    commit(
        &repo,
        "rescue.txt",
        "rescue\n",
        "Unchanged target branch",
        "2020-01-06T00:00:00Z",
    );
    git(repo.dir.path(), ["switch", "-"]);
    let second_merge = git_command(repo.dir.path())
        .args(["merge", "--no-ff", "--no-edit", "rescue"])
        .env("GIT_AUTHOR_DATE", "2020-01-07T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-07T00:00:00Z")
        .output()
        .expect("run second git merge");
    assert!(
        second_merge.status.success(),
        "second git merge failed: {}",
        String::from_utf8_lossy(&second_merge.stderr)
    );
    let endpoint = repo.head();
    repo.index();

    let report = json(
        &repo,
        &[
            "fate", "a.txt", "--line", "2", "--at", &start, "--to-rev", &endpoint, "--json",
        ],
    );

    assert_eq!(report["final_state"], "unknown");
    assert_eq!(report["stop_reason"]["code"], "line_rewritten");
    assert_eq!(report["stopped_at"], rewrite);
}

#[test]
fn fate_tracks_divergent_parent_coordinates_at_the_merge() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "alpha\nbeta\ngamma\n",
        "Base",
        "2020-01-01T00:00:00Z",
    );
    git(repo.dir.path(), ["switch", "-c", "side"]);
    commit(
        &repo,
        "a.txt",
        "side\nalpha\nbeta\ngamma\n",
        "Side insertion",
        "2020-01-02T00:00:00Z",
    );
    git(repo.dir.path(), ["switch", "-"]);
    commit(
        &repo,
        "a.txt",
        "alpha\nbeta\ngamma\nmain-tail\n",
        "Main insertion",
        "2020-01-03T00:00:00Z",
    );
    let merge = git_command(repo.dir.path())
        .args(["merge", "--no-ff", "--no-edit", "side"])
        .env("GIT_AUTHOR_DATE", "2020-01-04T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-04T00:00:00Z")
        .output()
        .expect("run git merge");
    assert!(
        merge.status.success(),
        "git merge failed: {}",
        String::from_utf8_lossy(&merge.stderr)
    );
    let endpoint = repo.head();
    repo.index();

    let report = json(
        &repo,
        &[
            "fate", "a.txt", "--line", "2", "--at", &start, "--to-rev", &endpoint, "--json",
        ],
    );

    assert_eq!(report["final_state"], "reached_endpoint");
    assert_eq!(report["last_location"]["line"], 3);
    let merge_event = report["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["commit_id"] == endpoint)
        .expect("merge event");
    assert_eq!(
        merge_event["incoming"],
        serde_json::json!([
            {"path": "a.txt", "line": 2},
            {"path": "a.txt", "line": 3}
        ])
    );
    let output = repo.run([
        "fate", "a.txt", "--line", "2", "--at", &start, "--to-rev", &endpoint,
    ]);
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("Predecessors:"), "{text}");
    assert!(text.contains("Incoming locations:"), "{text}");
}

#[test]
fn fate_display_truncation_preserves_edges_to_omitted_events() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "alpha\nbeta\n",
        "Base",
        "2020-01-01T00:00:00Z",
    );
    let mut path = String::from("a.txt");
    let mut first_move = None;
    let mut second_move = None;
    for index in 1..=21 {
        let next = format!("tracked-{index:02}.txt");
        let date = format!("2020-01-{:02}T00:00:00Z", index + 1);
        let movement = rename(&repo, &path, &next, "Rename tracked file", &date);
        match index {
            1 => first_move = Some(movement),
            2 => second_move = Some(movement),
            _ => {}
        }
        path = next;
    }
    repo.index();

    let report = json(
        &repo,
        &["fate", "a.txt", "--line", "2", "--at", &start, "--json"],
    );

    assert_eq!(report["final_state"], "reached_endpoint");
    assert_eq!(report["total_events"], 21);
    assert_eq!(report["display_truncated"], true);
    let events = report["events"].as_array().unwrap();
    assert_eq!(events.len(), 20);
    assert_eq!(events[0]["commit_id"], second_move.unwrap());
    assert_eq!(
        events[0]["predecessors"],
        serde_json::json!([first_move.unwrap()])
    );
}

#[test]
fn fate_tracks_symbol_correspondence_across_a_merge() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "lib.rs",
        "fn tracked() {\n    let value = 1;\n    value\n}\n",
        "Add tracked symbol",
        "2020-01-01T00:00:00Z",
    );
    git(repo.dir.path(), ["switch", "-c", "side"]);
    commit(
        &repo,
        "lib.rs",
        "fn side() {}\n\nfn tracked() {\n    let value = 1;\n    value\n}\n",
        "Add symbol before target",
        "2020-01-02T00:00:00Z",
    );
    git(repo.dir.path(), ["switch", "-"]);
    commit(
        &repo,
        "lib.rs",
        "fn tracked() {\n    let value = 1;\n    value\n}\n\nfn main_side() {}\n",
        "Add symbol after target",
        "2020-01-03T00:00:00Z",
    );
    let merge = git_command(repo.dir.path())
        .args(["merge", "--no-ff", "--no-edit", "side"])
        .env("GIT_AUTHOR_DATE", "2020-01-04T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-04T00:00:00Z")
        .output()
        .expect("run git merge");
    assert!(
        merge.status.success(),
        "git merge failed: {}",
        String::from_utf8_lossy(&merge.stderr)
    );
    let endpoint = repo.head();
    repo.index();

    let report = json(
        &repo,
        &[
            "fate", "lib.rs", "--symbol", "tracked", "--at", &start, "--to-rev", &endpoint,
            "--json",
        ],
    );

    assert_eq!(report["final_state"], "reached_endpoint");
    assert_eq!(report["last_location"]["line"], 3);
    assert_eq!(report["last_location"]["start_line"], 3);
    let merge_event = report["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["commit_id"] == endpoint)
        .expect("merge event");
    assert_eq!(merge_event["relationship"], "merge");
    assert_eq!(merge_event["incoming"].as_array().unwrap().len(), 2);
}

#[test]
fn fate_rejects_ambiguous_strict_move_at_a_merge() {
    let repo = TestRepo::new();
    let mut source_lines = (0..20)
        .map(|index| format!("before-{index}"))
        .collect::<Vec<_>>();
    let target_line = i64::try_from(source_lines.len() + 1).unwrap();
    source_lines.push("let tracked = 1;".to_owned());
    source_lines.extend((0..20).map(|index| format!("after-{index}")));
    let target_index = usize::try_from(target_line - 1).unwrap();
    let candidate = format!(
        "{}\n",
        source_lines[target_index - 5..target_index + 6].join("\n")
    );
    let source_without_target = source_lines
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != target_index)
        .map(|(_, line)| line.as_str())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let start = commit(
        &repo,
        "a.txt",
        &format!("{}\n", source_lines.join("\n")),
        "Add tracked line",
        "2020-01-01T00:00:00Z",
    );
    git(repo.dir.path(), ["switch", "-c", "side"]);
    fs::write(repo.dir.path().join("a.txt"), &source_without_target)
        .expect("remove tracked line from source file");
    fs::write(repo.dir.path().join("b.txt"), &candidate).expect("move target to side file");
    git(repo.dir.path(), ["add", "--all"]);
    let side_move = git_command(repo.dir.path())
        .args(["commit", "-m", "Move target on side"])
        .env("GIT_AUTHOR_DATE", "2020-01-02T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-02T00:00:00Z")
        .output()
        .expect("commit side move");
    assert!(
        side_move.status.success(),
        "{}",
        String::from_utf8_lossy(&side_move.stderr)
    );
    git(repo.dir.path(), ["switch", "-"]);
    commit(
        &repo,
        "other.txt",
        "unrelated\n",
        "Main change",
        "2020-01-03T00:00:00Z",
    );
    let merge = git_command(repo.dir.path())
        .args(["merge", "--no-ff", "--no-commit", "side"])
        .env("GIT_AUTHOR_DATE", "2020-01-04T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-04T00:00:00Z")
        .output()
        .expect("prepare merge");
    assert!(
        merge.status.success(),
        "{}",
        String::from_utf8_lossy(&merge.stderr)
    );
    fs::write(repo.dir.path().join("copy.txt"), &candidate).expect("add competing destination");
    git(repo.dir.path(), ["add", "--all"]);
    let merge_commit = git_command(repo.dir.path())
        .args(["commit", "-m", "Add competing merge destination"])
        .env("GIT_AUTHOR_DATE", "2020-01-04T00:00:01Z")
        .env("GIT_COMMITTER_DATE", "2020-01-04T00:00:01Z")
        .output()
        .expect("commit merge result");
    assert!(
        merge_commit.status.success(),
        "{}",
        String::from_utf8_lossy(&merge_commit.stderr)
    );
    let endpoint = repo.head();
    repo.index();

    let report = json(
        &repo,
        &[
            "fate",
            "a.txt",
            "--line",
            &target_line.to_string(),
            "--at",
            &start,
            "--to-rev",
            &endpoint,
            "--json",
        ],
    );

    assert_eq!(report["final_state"], "unknown");
    assert_eq!(report["stopped_at"], endpoint);
    assert_eq!(report["stop_reason"]["code"], "ambiguous_merge_move");
}

#[test]
fn fate_tracks_a_local_symbol_edit_across_a_merge() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "lib.rs",
        "fn tracked() {\n    let value = 1;\n    value\n}\n",
        "Add tracked symbol",
        "2020-01-01T00:00:00Z",
    );
    git(repo.dir.path(), ["switch", "-c", "side"]);
    let modification = commit(
        &repo,
        "lib.rs",
        "fn tracked() {\n    let value = 2;\n    value\n}\n",
        "Modify tracked symbol locally",
        "2020-01-02T00:00:00Z",
    );
    git(repo.dir.path(), ["switch", "-"]);
    commit(
        &repo,
        "other.txt",
        "unrelated\n",
        "Main change",
        "2020-01-03T00:00:00Z",
    );
    let merge = git_command(repo.dir.path())
        .args(["merge", "--no-ff", "--no-edit", "side"])
        .env("GIT_AUTHOR_DATE", "2020-01-04T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-04T00:00:00Z")
        .output()
        .expect("run Git merge");
    assert!(
        merge.status.success(),
        "{}",
        String::from_utf8_lossy(&merge.stderr)
    );
    let endpoint = repo.head();
    repo.index();

    let report = json(
        &repo,
        &[
            "fate", "lib.rs", "--symbol", "tracked", "--at", &start, "--to-rev", &endpoint,
            "--json",
        ],
    );

    assert_eq!(report["final_state"], "reached_endpoint");
    let events = report["events"].as_array().unwrap();
    assert!(events.iter().any(|event| {
        event["commit_id"] == modification && event["relationship"] == "modified"
    }));
    let merge_event = events
        .iter()
        .find(|event| event["commit_id"] == endpoint)
        .expect("merge event");
    assert_eq!(merge_event["relationship"], "merge");
    assert_eq!(merge_event["result_state"], "active");
    assert_eq!(merge_event["incoming"].as_array().unwrap().len(), 2);
}

#[test]
fn fate_tracks_a_strict_symbol_move_through_a_merge() {
    let repo = TestRepo::new();
    let source = "fn tracked() {\n    let value = 1;\n    value\n}\n";
    let start = commit(
        &repo,
        "a.rs",
        source,
        "Add tracked symbol",
        "2020-01-01T00:00:00Z",
    );
    git(repo.dir.path(), ["switch", "-c", "side"]);
    fs::remove_file(repo.dir.path().join("a.rs")).expect("remove old symbol file");
    fs::write(repo.dir.path().join("b.rs"), source).expect("move symbol to new file");
    git(repo.dir.path(), ["add", "--all"]);
    let side_move = git_command(repo.dir.path())
        .args(["commit", "-m", "Move tracked symbol"])
        .env("GIT_AUTHOR_DATE", "2020-01-02T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-02T00:00:00Z")
        .output()
        .expect("commit symbol move");
    assert!(
        side_move.status.success(),
        "{}",
        String::from_utf8_lossy(&side_move.stderr)
    );
    git(repo.dir.path(), ["switch", "-"]);
    commit(
        &repo,
        "other.txt",
        "unrelated\n",
        "Main change",
        "2020-01-03T00:00:00Z",
    );
    let merge = git_command(repo.dir.path())
        .args(["merge", "--no-ff", "--no-edit", "side"])
        .env("GIT_AUTHOR_DATE", "2020-01-04T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-04T00:00:00Z")
        .output()
        .expect("merge symbol move");
    assert!(
        merge.status.success(),
        "{}",
        String::from_utf8_lossy(&merge.stderr)
    );
    let endpoint = repo.head();
    repo.index();

    let report = json(
        &repo,
        &[
            "fate", "a.rs", "--symbol", "tracked", "--at", &start, "--to-rev", &endpoint, "--json",
        ],
    );

    assert_eq!(report["final_state"], "reached_endpoint", "{report:#}");
    assert_eq!(report["last_location"]["path"], "b.rs");
    let merge_event = report["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["commit_id"] == endpoint)
        .expect("merge event");
    assert_eq!(merge_event["relationship"], "merge");
    assert_eq!(merge_event["after"]["path"], "b.rs");
    assert_eq!(merge_event["incoming"].as_array().unwrap().len(), 2);
}

#[test]
fn fate_excludes_descendants_after_the_requested_endpoint() {
    let repo = TestRepo::new();
    let start = commit(
        &repo,
        "a.txt",
        "tracked\n",
        "Add tracked line",
        "2020-01-01T00:00:00Z",
    );
    let endpoint = commit(
        &repo,
        "a.txt",
        "rewritten\n",
        "Rewrite tracked line",
        "2020-01-02T00:00:00Z",
    );
    let descendant = commit(
        &repo,
        "unrelated.txt",
        "unrelated\n",
        "Unrelated descendant",
        "2020-01-03T00:00:00Z",
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "fate", "a.txt", "--line", "1", "--at", &start, "--to-rev", &endpoint, "--json",
        ],
    );

    assert_eq!(report["final_state"], "unknown");
    assert_eq!(report["total_events"], 1);
    let events = report["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["commit_id"], endpoint);
    assert_ne!(events[0]["commit_id"], descendant);
}
