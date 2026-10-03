mod support;

use rusqlite::Connection;
use serde_json::Value;
use std::fs;
use support::{TestRepo, git, git_command, git_stdout};

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
    let uncached_head = repo.head();
    let after_uncached = query(&repo, &[]);
    assert_eq!(after_uncached["events"], report["events"]);
    assert_eq!(after_uncached["scope"]["to_rev"], uncached_head);
    assert_eq!(after_uncached["scope"]["cache_tip"], report["cache_tip"]);
    assert_eq!(after_uncached["scope"]["coverage_complete"], false);
    assert!(
        after_uncached["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| {
                let warning = warning.as_str().unwrap();
                warning.contains("incomplete history coverage")
                    && warning.contains("run `gitscry index`")
            })
    );
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
    let rename_patch = &report["events"][0]["patch"];
    assert_eq!(rename_patch["status"], "available");
    let rename_hunk = &rename_patch["hunks"][0];
    assert_eq!(rename_hunk["old_path"], "old.rs");
    assert_eq!(rename_hunk["new_path"], "new.rs");
    assert!(rename_hunk["text"].as_str().unwrap().contains("-old()"));
    assert!(rename_hunk["text"].as_str().unwrap().contains("+new()"));
    assert_eq!(query(&repo, &["--path", "new.rs"])["matched_count"], 0);
    let deleted = query(&repo, &["--path", "deleted.rs"]);
    assert_eq!(deleted["events"][0]["file_change"]["status"], "D");
    assert_eq!(deleted["events"][0]["patch"]["status"], "available");
    assert_eq!(
        deleted["events"][0]["patch"]["hunks"][0]["old_path"],
        "deleted.rs"
    );
    assert!(
        deleted["events"][0]["patch"]["hunks"][0]["text"]
            .as_str()
            .unwrap()
            .contains("-old()")
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

#[test]
fn returns_bounded_patch_fragments_around_matching_deleted_lines() {
    let repo = TestRepo::new();
    let mut before = String::from("anchor\nkeep1\nkeep2\nold()\nnear\nold()\n");
    let mut after = String::from("anchor\nkeep1\n");
    for index in 0..600 {
        after.push_str(&format!("added_{index:04}_padding_value\n"));
    }
    after.push_str("keep2\nnear\n");
    for index in 0..40 {
        let line = format!("padding_{index}\n");
        before.push_str(&line);
        after.push_str(&line);
    }
    before.push_str("old()\nend\n");
    after.push_str("end\n");
    commit(
        &repo,
        &[("a.rs", &before)],
        "Introduce old lines",
        "2000-01-01T00:00:00Z",
    );
    commit(
        &repo,
        &[("a.rs", &after)],
        "Remove old lines after large addition",
        "2000-01-02T00:00:00Z",
    );
    repo.index();

    let report = query(&repo, &[]);
    let event = &report["events"][0];
    assert_eq!(event["matches"].as_array().unwrap().len(), 3);
    let patch = &event["patch"];
    assert_eq!(patch["status"], "available");
    assert_eq!(patch["truncated"], false);
    let hunks = patch["hunks"].as_array().unwrap();
    assert_eq!(hunks.len(), 2);
    let first = hunks[0]["text"].as_str().unwrap();
    assert!(first.contains("-old()"));
    assert!(first.contains("+added_0599_padding_value"));
    assert!(!first.contains("+added_0000_padding_value"));
    assert!(hunks[1]["text"].as_str().unwrap().contains("-old()"));

    let human = repo.run(["trace-removal", "--code", "old()"]).stdout;
    let human = String::from_utf8(human).unwrap();
    assert!(human.contains("patch excerpt: available"));
    assert!(human.contains("+added_0599_padding_value"));
    assert!(human.contains("-old()"));
}

#[test]
fn prioritizes_matching_hunk_after_unrelated_cached_hunks() {
    let repo = TestRepo::new();
    let mut before = String::new();
    let mut after = String::new();
    for index in 0..70 {
        before.push_str(&format!("removed_{index}\n"));
        after.push_str(&format!("replacement_{index}\n"));
        for context in 0..10 {
            let line = format!("context_{index}_{context}\n");
            before.push_str(&line);
            after.push_str(&line);
        }
    }
    before.push_str("old()\nend\n");
    after.push_str("end\n");
    commit(
        &repo,
        &[("a.rs", &before)],
        "Introduce old line",
        "2000-01-01T00:00:00Z",
    );
    commit(
        &repo,
        &[("a.rs", &after)],
        "Remove after many unrelated hunks",
        "2000-01-02T00:00:00Z",
    );
    repo.index();

    let old_blob = git_stdout(repo.dir.path(), ["rev-parse", "HEAD~1:a.rs"]);
    let loose_blob = repo
        .dir
        .path()
        .join(".git/objects")
        .join(&old_blob[..2])
        .join(&old_blob[2..]);
    fs::remove_file(loose_blob).unwrap();

    let report = query(&repo, &[]);
    let event = &report["events"][0];
    assert_eq!(event["matches"].as_array().unwrap().len(), 1);
    let patch = &event["patch"];
    assert_eq!(patch["status"], "available");
    assert_eq!(patch["truncated"], false);
    let hunks = patch["hunks"].as_array().unwrap();
    assert_eq!(hunks.len(), 1);
    let text = hunks[0]["text"].as_str().unwrap();
    assert!(text.contains("-old()"));
    assert!(!text.contains("replacement_0\n"));
    assert!(!text.contains("replacement_69\n"));
}

#[test]
fn keeps_event_matches_when_cached_patch_text_exceeds_the_budget() {
    let repo = TestRepo::new();
    let parent = commit(
        &repo,
        &[("a.rs", "old()\nend\n")],
        "Introduce old line",
        "2000-01-01T00:00:00Z",
    );
    let mut after = String::new();
    for index in 0..3000 {
        after.push_str(&format!("added_{index:04}_padding_value\n"));
    }
    after.push_str("end\n");
    commit(
        &repo,
        &[("a.rs", &after)],
        "Remove with oversized patch",
        "2000-01-02T00:00:00Z",
    );
    repo.index();

    let report = query(&repo, &[]);
    let event = &report["events"][0];
    assert_eq!(event["first_parent_id"], parent);
    assert_eq!(event["matches"].as_array().unwrap().len(), 1);
    assert_eq!(event["old_path"], "a.rs");
    assert_eq!(event["patch"]["status"], "unavailable");
    assert_eq!(event["patch"]["truncated"], true);
    let human = repo.run(["trace-removal", "--code", "old()"]).stdout;
    let human = String::from_utf8(human).unwrap();
    assert!(human.contains("Text hunk unavailable."));
    assert!(human.contains("patch excerpt truncated by safety limits."));
}

#[test]
fn excerpt_headers_track_both_sides_after_omitted_rows() {
    let repo = TestRepo::new();
    let prefix = "prefix_1\nprefix_2\nprefix_3\nprefix_4\n";
    let addition_first_before = format!("{prefix}anchor\nold()\ntail\n");
    let addition_first_after = format!("{prefix}added_1\nadded_2\nadded_3\nanchor\ntail\n");
    let deletion_first_before = format!("{prefix}remove_1\nremove_2\nremove_3\nold()\ntail\n");
    let deletion_first_after = format!("{prefix}added\ntail\n");
    commit(
        &repo,
        &[
            ("addition_first.rs", &addition_first_before),
            ("deletion_first.rs", &deletion_first_before),
        ],
        "Introduce old lines",
        "2000-01-01T00:00:00Z",
    );
    commit(
        &repo,
        &[
            ("addition_first.rs", &addition_first_after),
            ("deletion_first.rs", &deletion_first_after),
        ],
        "Remove old lines",
        "2000-01-02T00:00:00Z",
    );
    repo.index();

    let report = query(&repo, &[]);
    let events = report["events"].as_array().unwrap();
    assert_eq!(events.len(), 2);
    let addition_first = events
        .iter()
        .find(|event| event["old_path"] == "addition_first.rs")
        .unwrap();
    let addition_hunk = &addition_first["patch"]["hunks"][0];
    assert_eq!(addition_hunk["old_start"], 5);
    assert_eq!(addition_hunk["old_lines"], 3);
    assert_eq!(addition_hunk["new_start"], 6);
    assert_eq!(addition_hunk["new_lines"], 4);
    let addition_text = addition_hunk["text"].as_str().unwrap();
    assert_eq!(addition_text.lines().next(), Some("@@ -5,3 +6,4 @@"));
    assert!(addition_text.lines().nth(1).unwrap().starts_with('+'));

    let deletion_first = events
        .iter()
        .find(|event| event["old_path"] == "deletion_first.rs")
        .unwrap();
    let deletion_hunk = &deletion_first["patch"]["hunks"][0];
    assert_eq!(deletion_hunk["old_start"], 5);
    assert_eq!(deletion_hunk["old_lines"], 5);
    assert_eq!(deletion_hunk["new_start"], 5);
    assert_eq!(deletion_hunk["new_lines"], 2);
    let deletion_text = deletion_hunk["text"].as_str().unwrap();
    assert_eq!(deletion_text.lines().next(), Some("@@ -5,5 +5,2 @@"));
    assert!(deletion_text.lines().nth(1).unwrap().starts_with('-'));
}

#[test]
fn same_commit_navigation_is_per_event_and_lists_detected_changes() {
    let repo = TestRepo::new();
    commit(
        &repo,
        &[
            ("a.rs", "old()\n"),
            ("b.rs", "old()\n"),
            ("modified.txt", "before\n"),
            ("rename-old.txt", "rename body\n"),
        ],
        "Root",
        "2000-01-01T00:00:00Z",
    );
    fs::remove_file(repo.dir.path().join("rename-old.txt")).unwrap();
    let removal = commit(
        &repo,
        &[
            ("a.rs", "new()\n"),
            ("b.rs", "new()\n"),
            ("modified.txt", "after\n"),
            ("added.txt", "added\n"),
            ("rename-new.txt", "rename body\n"),
        ],
        "Remove in several files",
        "2000-01-02T00:00:00Z",
    );
    repo.index();

    let report = query(&repo, &[]);
    assert_eq!(report, query(&repo, &[]));
    assert_eq!(report["events"].as_array().unwrap().len(), 2);
    let a_files = &report["events"][0]["same_commit_files"];
    assert_eq!(report["events"][0]["old_path"], "a.rs");
    assert_eq!(report["events"][0]["commit_id"], removal);
    assert_eq!(a_files["status"], "complete");
    assert_eq!(
        a_files["files"],
        serde_json::json!([
            {"status": "A", "old_path": null, "new_path": "added.txt"},
            {"status": "M", "old_path": "b.rs", "new_path": "b.rs"},
            {"status": "M", "old_path": "modified.txt", "new_path": "modified.txt"},
            {"status": "R100", "old_path": "rename-old.txt", "new_path": "rename-new.txt"}
        ])
    );

    let b_files = &report["events"][1]["same_commit_files"];
    assert_eq!(report["events"][1]["old_path"], "b.rs");
    assert_eq!(b_files["status"], "complete");
    assert_eq!(
        b_files["files"],
        serde_json::json!([
            {"status": "M", "old_path": "a.rs", "new_path": "a.rs"},
            {"status": "A", "old_path": null, "new_path": "added.txt"},
            {"status": "M", "old_path": "modified.txt", "new_path": "modified.txt"},
            {"status": "R100", "old_path": "rename-old.txt", "new_path": "rename-new.txt"}
        ])
    );
    assert_eq!(report["truncated"], false);
    assert_eq!(
        report["notices"][0],
        "Same-commit co-changes are navigation only; they do not prove replacement, migration intent, causality, or cross-file identity."
    );

    let human = repo.run(["trace-removal", "--code", "old()"]);
    assert!(human.status.success());
    assert!(String::from_utf8_lossy(&human.stdout).contains("Same-commit file navigation"));
    assert!(String::from_utf8_lossy(&human.stdout).contains("do not prove replacement"));
}

#[test]
fn same_commit_navigation_distinguishes_empty_and_truncated_summaries() {
    let empty_repo = TestRepo::new();
    commit(
        &empty_repo,
        &[("gone.rs", "old()\n")],
        "Root",
        "2000-01-01T00:00:00Z",
    );
    fs::remove_file(empty_repo.dir.path().join("gone.rs")).unwrap();
    commit(&empty_repo, &[], "Remove only file", "2000-01-02T00:00:00Z");
    empty_repo.index();
    let empty_report = query(&empty_repo, &[]);
    assert_eq!(
        empty_report["events"][0]["same_commit_files"]["status"],
        "complete"
    );
    assert_eq!(
        empty_report["events"][0]["same_commit_files"]["files"],
        serde_json::json!([])
    );

    let human = empty_repo.run(["trace-removal", "--code", "old()"]);
    assert!(human.status.success());
    let text = String::from_utf8_lossy(&human.stdout);
    assert!(text.contains("Same-commit file navigation: complete"));
    assert!(text.contains("No other files changed."));

    let repo = TestRepo::new();
    commit(
        &repo,
        &[("gone.rs", "old()\n")],
        "Root",
        "2000-01-01T00:00:00Z",
    );
    fs::remove_file(repo.dir.path().join("gone.rs")).unwrap();
    let files: Vec<_> = (0..13)
        .map(|index| {
            (
                format!("other-{index:02}.txt"),
                format!("content {index}\n"),
            )
        })
        .collect();
    let file_refs: Vec<_> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    commit(
        &repo,
        &file_refs,
        "Remove with many co-changes",
        "2000-01-02T00:00:00Z",
    );
    repo.index();

    let report = query(&repo, &[]);
    let summary = &report["events"][0]["same_commit_files"];
    assert_eq!(summary["status"], "truncated");
    assert_eq!(summary["files"].as_array().unwrap().len(), 12);
    assert_eq!(summary["files"][0]["new_path"], "other-00.txt");
    assert_eq!(summary["files"][11]["new_path"], "other-11.txt");
    assert_eq!(report["truncated"], false);
    let human = repo.run(["trace-removal", "--code", "old()"]);
    assert!(human.status.success());
    assert!(
        String::from_utf8_lossy(&human.stdout).contains("Same-commit file navigation: truncated")
    );
}

#[test]
fn supplemental_lookup_failure_preserves_the_event_and_reports_unavailable() {
    let repo = TestRepo::new();
    let root = commit(
        &repo,
        &[("gone.rs", "old()\n")],
        "Root",
        "2000-01-01T00:00:00Z",
    );
    fs::remove_file(repo.dir.path().join("gone.rs")).unwrap();
    let removal = commit(
        &repo,
        &[("added.txt", "")],
        "Remove with an empty co-change",
        "2000-01-02T00:00:00Z",
    );
    repo.index();

    // Make only the supplemental row's status unreadable; event discovery still works.
    let connection = Connection::open(repo.dir.path().join(".gitscry/cache.sqlite")).unwrap();
    connection
        .execute_batch(
            "ALTER TABLE changes RENAME TO cached_changes;
             CREATE VIEW changes AS
             SELECT change_id, commit_id, ordinal,
                    CASE WHEN old_path IS NULL THEN 1 ELSE status END AS status,
                    old_path, new_path, old_blob, new_blob, old_mode, new_mode
             FROM cached_changes;",
        )
        .unwrap();
    drop(connection);

    let report = query(&repo, &[]);
    assert_eq!(report["events"].as_array().unwrap().len(), 1);
    let event = &report["events"][0];
    assert_eq!(event["commit_id"], removal);
    assert_eq!(event["first_parent_id"], root);
    assert_eq!(event["old_path"], "gone.rs");
    assert_eq!(event["same_commit_files"]["status"], "unavailable");
    assert_eq!(
        event["matches"],
        serde_json::json!([{"line_number": 1, "line": "old()"}])
    );
    assert_eq!(event["same_commit_files"]["files"], serde_json::json!([]));
    assert_eq!(report["truncated"], false);

    let human = repo.run(["trace-removal", "--code", "old()"]);
    assert!(human.status.success());
    let text = String::from_utf8_lossy(&human.stdout);
    assert!(text.contains(&removal));
    assert!(text.contains(&format!("Preceding file locator: {root}:gone.rs")));
    assert!(text.contains("Same-commit file navigation: unavailable"));
}
