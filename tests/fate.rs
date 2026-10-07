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
    assert_eq!(report["schema_version"], 1);
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
    assert_eq!(report["schema_version"], 2);
    let text_output = repo.run(["fate", "a.txt", "--line", "2", "--at", &start, "--patch"]);
    assert_eq!(text_output.status.code(), Some(0));
    let text = String::from_utf8_lossy(&text_output.stdout);
    assert!(text.contains("patch excerpt: available"), "{text}");
    assert!(text.contains("-beta"), "{text}");
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
    assert!(!text.contains("unrelated edit"), "{text}");
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
    assert_eq!(patched["schema_version"], 2);
    assert_eq!(plain["schema_version"], 1);
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
