mod support;

use serde_json::Value;
use std::fs;
use support::{TestRepo, git, git_command};

fn commit(repo: &TestRepo, path: &str, content: &str, subject: &str, date: &str) -> String {
    fs::write(repo.dir.path().join(path), content).unwrap();
    commit_all(repo, subject, date)
}

fn commit_all(repo: &TestRepo, subject: &str, date: &str) -> String {
    git(repo.dir.path(), ["add", "--all"]);
    let out = git_command(repo.dir.path())
        .args(["commit", "-m", subject])
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    repo.head()
}

fn json(repo: &TestRepo, args: &[&str]) -> Value {
    let out = repo.run(args);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn followups_reports_only_inspected_same_file_material() {
    let repo = TestRepo::new();
    let seed = commit(&repo, "a", "seed\n", "Seed", "2020-01-01T00:00:00Z");
    commit(
        &repo,
        "other",
        "other\n",
        "Nonmatch",
        "2020-01-02T00:00:00Z",
    );
    let early = commit(&repo, "a", "early\n", "Early", "2019-12-31T00:00:00Z");
    let later = commit(&repo, "a", "later\n", "Later", "2020-01-03T00:00:00Z");
    repo.index();
    let report = json(&repo, &["followups", &seed, "--max-commits", "2", "--json"]);
    assert_eq!(report["kind"], "followups");
    assert_eq!(report["inspected_count"], 2);
    assert_eq!(report["traversal_truncated"], true);
    assert_eq!(report["display_truncated"], false);
    assert_eq!(report["entries"].as_array().unwrap().len(), 1);
    assert_eq!(report["entries"][0]["commit_id"], early);
    assert_eq!(report["entries"][0]["elapsed_seconds"], -86400);
    assert_eq!(report["entries"][0]["basis"], "same_file");
    assert!(!report["warnings"].as_array().unwrap().is_empty());
    let report = json(&repo, &["followups", &seed, "--limit", "1", "--json"]);
    assert_eq!(report["inspected_count"], 3);
    assert_eq!(report["matched_in_inspected_scope"], 2);
    assert_eq!(report["display_truncated"], true);
    assert_eq!(report["traversal_truncated"], false);
    assert_eq!(report["scope"]["endpoint"], later);
    let out = repo.run(["followups", &seed]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains(&early) && text.contains("same_file") && text.contains("-86400"));
    let empty = json(&repo, &["followups", &seed, "--to-rev", &seed, "--json"]);
    assert_eq!(empty["inspected_count"], 0);
    assert!(empty["entries"].as_array().unwrap().is_empty());
}

#[test]
fn followups_paths_incarnations_and_bounded_patch() {
    let repo = TestRepo::new();
    let seed = commit(&repo, "a", "old\n", "Seed", "2020-01-01T00:00:00Z");
    let changed = commit(&repo, "a", "new\n", "Modify", "2020-01-02T00:00:00Z");
    fs::remove_file(repo.dir.path().join("a")).unwrap();
    git(repo.dir.path(), ["add", "--all"]);
    git(repo.dir.path(), ["commit", "-m", "Delete"]);
    commit(
        &repo,
        "a",
        "recreated\n",
        "Recreate",
        "2020-01-03T00:00:00Z",
    );
    commit(
        &repo,
        "a",
        "recreated edit\n",
        "Unrelated incarnation",
        "2020-01-04T00:00:00Z",
    );
    repo.index();
    let report = json(&repo, &["followups", &seed, "--path", "a", "--json"]);
    assert_eq!(report["entries"].as_array().unwrap().len(), 1);
    assert_eq!(report["entries"][0]["commit_id"], changed);
    assert!(report["entries"][0].get("patch").is_none());
    let report = json(
        &repo,
        &["followups", &seed, "--path", "a", "--patch", "--json"],
    );
    let patch = &report["entries"][0]["patch"];
    assert_eq!(patch["status"], "available");
    assert_eq!(patch["hunks"][0]["text"], "@@ -1 +1 @@\n-old\n+new\n");
    for path in ["absent", "../a", "/a"] {
        let out = repo.run(["followups", &seed, "--path", path]);
        assert!(!out.status.success());
    }
}

#[test]
fn followups_validates_inputs_and_explains_scope_in_help() {
    let repo = TestRepo::new();
    let seed = commit(&repo, "a", "seed\n", "Seed", "2020-01-01T00:00:00Z");
    assert!(!repo.run(["followups", &seed]).status.success()); // no implicit index
    repo.index();
    for option in ["--days", "--max-commits", "--limit"] {
        for value in [
            "0",
            "-1",
            "garbage",
            "99999999999999999999999999999999999999",
        ] {
            assert!(
                !repo
                    .run(["followups", &seed, option, value])
                    .status
                    .success()
            );
        }
    }
    assert!(
        !repo
            .run(["followups", &seed, "--days", "18446744073709551615"])
            .status
            .success()
    );
    assert!(!repo.run(["followups", "not-a-ref"]).status.success());
    let unindexed = commit(
        &repo,
        "a",
        "unindexed\n",
        "Outside cache",
        "2020-01-02T00:00:00Z",
    );
    assert!(!repo.run(["followups", &unindexed]).status.success());
    assert!(
        !repo
            .run(["followups", &seed, "--to-rev", &unindexed])
            .status
            .success()
    );
    let help = repo.run(["followups", "--help"]);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    for word in [
        "90",
        "2000",
        "20",
        "positive",
        "inclusive",
        "ancestor",
        "same-file",
        "rename",
        "--patch",
        "--days 180",
    ] {
        assert!(help.contains(word), "help omitted {word}");
    }
    let top = String::from_utf8(repo.run(["--help"]).stdout).unwrap();
    assert!(!top.contains("out-of-window") && !top.contains("--days 180"));
}

#[test]
fn followups_window_is_inclusive_without_a_lower_time_bound() {
    let repo = TestRepo::new();
    let seed = commit(&repo, "a", "seed\n", "Seed", "2020-01-01T00:00:00Z");
    let boundary = commit(&repo, "a", "boundary\n", "Boundary", "2020-01-02T00:00:00Z");
    commit(&repo, "a", "outside\n", "Outside", "2020-01-02T00:00:01Z");
    let inverted = commit(&repo, "a", "inverted\n", "Inverted", "2019-12-31T00:00:00Z");
    repo.index();
    let report = json(&repo, &["followups", &seed, "--days", "1", "--json"]);
    assert_eq!(report["inspected_count"], 2);
    assert_eq!(report["lineage_inspected_count"], 1);
    assert_eq!(report["entries"][0]["commit_id"], boundary);
    assert_eq!(report["entries"][1]["commit_id"], inverted);
    assert_eq!(report["entries"][1]["elapsed_seconds"], -86400);
    assert_eq!(report["traversal_truncated"], false);
}

#[test]
fn followups_seed_rename_deletion_and_later_rename_stop_identity() {
    let repo = TestRepo::new();
    commit(
        &repo,
        "old",
        "original\n",
        "Original",
        "2020-01-01T00:00:00Z",
    );
    git(repo.dir.path(), ["mv", "old", "new"]);
    let seed = commit_all(&repo, "Seed rename", "2020-01-02T00:00:00Z");
    let edit = commit(&repo, "new", "edit\n", "Edit", "2020-01-03T00:00:00Z");
    git(repo.dir.path(), ["mv", "new", "later"]);
    let rename = commit_all(&repo, "Subsequent rename", "2020-01-04T00:00:00Z");
    commit(
        &repo,
        "new",
        "unrelated\n",
        "Recreate old path",
        "2020-01-05T00:00:00Z",
    );
    commit(
        &repo,
        "later",
        "next\n",
        "Unsupported rename continuation",
        "2020-01-06T00:00:00Z",
    );
    repo.index();
    let old = json(&repo, &["followups", &seed, "--path", "old", "--json"]);
    let new = json(
        &repo,
        &[
            "followups",
            &seed,
            "--path",
            "new",
            "--path",
            "old",
            "--json",
        ],
    );
    assert_eq!(old["entries"], new["entries"]);
    assert_eq!(old["entries"].as_array().unwrap().len(), 2);
    assert_eq!(old["entries"][0]["commit_id"], edit);
    assert_eq!(old["entries"][1]["commit_id"], rename);
    assert!(old["warnings"].to_string().contains("tracking stopped"));
    fs::remove_file(repo.dir.path().join("later")).unwrap();
    let deletion = commit_all(&repo, "Seed deletion", "2020-01-07T00:00:00Z");
    commit(
        &repo,
        "later",
        "recreated\n",
        "Recreate",
        "2020-01-08T00:00:00Z",
    );
    repo.index();
    let empty = json(&repo, &["followups", &deletion, "--json"]);
    assert_eq!(empty["inspected_count"], 1);
    assert!(empty["entries"].as_array().unwrap().is_empty());
    // A rename away is an ending association, never a resurrection.
    let report = json(&repo, &["followups", &edit, "--path", "new", "--json"]);
    assert_eq!(report["entries"].as_array().unwrap().len(), 1);
}

#[test]
fn followups_excludes_parallel_work_and_discloses_ambiguous_merges() {
    let repo = TestRepo::new();
    let base = commit(&repo, "a", "base\n", "Base", "2020-01-01T00:00:00Z");
    git(repo.dir.path(), ["checkout", "-b", "parallel"]);
    let parallel = commit(&repo, "a", "parallel\n", "Parallel", "2020-01-03T00:00:00Z");
    git(repo.dir.path(), ["checkout", "-b", "seed-branch", &base]);
    let seed = commit(&repo, "a", "seed\n", "Seed", "2020-01-02T00:00:00Z");
    let edit = commit(&repo, "a", "edit\n", "Early edit", "2020-01-04T00:00:00Z");
    let out = git_command(repo.dir.path())
        .args(["merge", "-s", "ours", "--no-ff", "parallel", "-m", "Merge"])
        .env("GIT_AUTHOR_DATE", "2020-01-05T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-05T00:00:00Z")
        .output()
        .unwrap();
    assert!(out.status.success());
    commit(
        &repo,
        "a",
        "later\n",
        "Ambiguous continuation",
        "2020-01-06T00:00:00Z",
    );
    git(repo.dir.path(), ["branch", "-f", "main", "HEAD"]);
    repo.index();
    let report = json(&repo, &["followups", &seed, "--json"]);
    assert_eq!(report["inspected_count"], 3);
    assert_eq!(report["entries"].as_array().unwrap().len(), 1);
    assert_eq!(report["entries"][0]["commit_id"], edit);
    assert!(report["warnings"].to_string().contains("ambiguous merge"));
    assert!(
        !repo
            .run(["followups", &seed, "--to-rev", &parallel])
            .status
            .success()
    );
}

#[test]
fn followups_accepts_merge_seed_and_merge_results() {
    let repo = TestRepo::new();
    let seed = commit(&repo, "a", "seed\n", "Seed", "2020-01-01T00:00:00Z");
    git(repo.dir.path(), ["checkout", "-b", "topic"]);
    commit(&repo, "a", "topic\n", "Topic", "2020-01-02T00:00:00Z");
    git(repo.dir.path(), ["checkout", "-b", "mainline", &seed]);
    commit(&repo, "b", "main\n", "Nonmatch", "2020-01-03T00:00:00Z");
    let out = git_command(repo.dir.path())
        .args(["merge", "--no-ff", "topic", "-m", "Merge"])
        .env("GIT_AUTHOR_DATE", "2020-01-04T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-04T00:00:00Z")
        .output()
        .unwrap();
    assert!(out.status.success());
    let merge = repo.head();
    let edit = commit(&repo, "a", "edit\n", "After merge", "2020-01-05T00:00:00Z");
    git(repo.dir.path(), ["branch", "-f", "main", "HEAD"]);
    repo.index();
    let report = json(&repo, &["followups", &seed, "--json"]);
    let merge_entry = report["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["commit_id"] == merge)
        .unwrap();
    assert_eq!(merge_entry["diff_comparison"], "first_parent");
    let report = json(&repo, &["followups", &merge, "--path", "a", "--json"]);
    assert_eq!(report["entries"][0]["commit_id"], edit);
}

#[test]
fn followups_patch_bytes_are_bounded_and_binary_is_unavailable() {
    let repo = TestRepo::new();
    let seed = commit(&repo, "a", "seed\n", "Seed", "2020-01-01T00:00:00Z");
    commit(
        &repo,
        "a",
        &"x".repeat(20000),
        "Large change",
        "2020-01-02T00:00:00Z",
    );
    commit(&repo, "a", "binary\0new", "Binary", "2020-01-03T00:00:00Z");
    repo.index();
    let report = json(&repo, &["followups", &seed, "--patch", "--json"]);
    let large = &report["entries"][0]["patch"];
    assert_eq!(large["truncated"], true);
    assert!(large["hunks"][0]["text"].as_str().unwrap().len() <= 8192);
    assert_eq!(report["entries"][1]["patch"]["status"], "unavailable");
    let out = repo.run(["followups", &seed, "--patch"]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("truncated") && text.contains("unavailable"));
}

#[test]
fn followups_deletion_ends_identity_and_preserves_shallow_coverage_read_only() {
    let repo = TestRepo::new();
    let boundary = commit(&repo, "a", "before\n", "Boundary", "2019-12-31T00:00:00Z");
    let seed = commit(&repo, "a", "seed\n", "Seed", "2020-01-01T00:00:00Z");
    commit(
        &repo,
        "copy",
        "seed\n",
        "Copy unrelated file",
        "2020-01-02T00:00:00Z",
    );
    fs::remove_file(repo.dir.path().join("a")).unwrap();
    let deletion = commit_all(&repo, "Delete", "2020-01-03T00:00:00Z");
    commit(&repo, "a", "recreate\n", "Recreate", "2020-01-04T00:00:00Z");
    commit(
        &repo,
        "a",
        "edit\n",
        "Edit recreated file",
        "2020-01-05T00:00:00Z",
    );
    fs::write(
        repo.dir.path().join(".git/shallow"),
        format!("{boundary}\n"),
    )
    .unwrap();
    repo.index();
    let cache_path = repo.dir.path().join(".gitscry/cache.sqlite");
    let before = fs::read(&cache_path).unwrap();
    let report = json(&repo, &["followups", &seed, "--json"]);
    assert_eq!(report["inspected_count"], 4);
    assert_eq!(report["entries"].as_array().unwrap().len(), 1);
    assert_eq!(report["entries"][0]["commit_id"], deletion);
    assert_eq!(report["entries"][0]["change_types"][0], "D");
    assert!(report["warnings"].to_string().contains("shallow"));
    assert_eq!(before, fs::read(cache_path).unwrap());
    let out = repo.run(["followups", &seed]);
    assert!(String::from_utf8(out.stderr).unwrap().contains("shallow"));
}

#[test]
fn followups_discloses_out_of_window_lineage_budget() {
    let repo = TestRepo::new();
    let seed = commit(&repo, "a", "seed\n", "Seed", "2020-01-01T00:00:00Z");
    commit(
        &repo,
        "a",
        "outside 1\n",
        "Outside 1",
        "2020-01-04T00:00:00Z",
    );
    commit(
        &repo,
        "a",
        "outside 2\n",
        "Outside 2",
        "2020-01-05T00:00:00Z",
    );
    commit(&repo, "a", "inverted\n", "Inverted", "2020-01-02T00:00:00Z");
    repo.index();
    let report = json(
        &repo,
        &[
            "followups",
            &seed,
            "--days",
            "1",
            "--max-commits",
            "1",
            "--json",
        ],
    );
    assert_eq!(report["inspected_count"], 0);
    assert_eq!(report["lineage_inspected_count"], 1);
    assert_eq!(report["traversal_truncated"], true);
    assert_eq!(report["matched_in_inspected_scope"], 0);
    assert!(report["warnings"].to_string().contains("lineage budget"));
}

#[test]
fn followups_prioritizes_explicit_revert_references_and_keeps_bases_separate() {
    let repo = TestRepo::new();
    fs::write(repo.dir.path().join("a"), "seed a\n").unwrap();
    fs::write(repo.dir.path().join("b"), "seed b\n").unwrap();
    let seed = commit_all(&repo, "Seed", "2020-01-01T00:00:00Z");

    let generic = commit(
        &repo,
        "a",
        "generic revert contents\n",
        "Revert \"Seed\"",
        "2020-01-02T00:00:00Z",
    );
    let explicit_other_path = commit(
        &repo,
        "b",
        "other path changed\n",
        &format!("Revert unrelated title\n\nThis reverts commit {seed}."),
        "2020-01-03T00:00:00Z",
    );
    let empty_message = format!("Revert without a diff\n\nThis reverts commit {seed}.");
    let empty_out = git_command(repo.dir.path())
        .args(["commit", "--allow-empty", "-m", &empty_message])
        .env("GIT_AUTHOR_DATE", "2020-01-04T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-04T00:00:00Z")
        .output()
        .unwrap();
    assert!(
        empty_out.status.success(),
        "{}",
        String::from_utf8_lossy(&empty_out.stderr)
    );
    let explicit_no_paths = repo.head();
    let explicit_same_file = commit(
        &repo,
        "a",
        "referenced path changed\n",
        &format!("Revert with path change\n\nThis reverts commit {seed}."),
        "2020-01-05T00:00:00Z",
    );
    let generic_same_file = commit(
        &repo,
        "a",
        "generic revert path change\n",
        "Revert \"Seed\"",
        "2020-01-06T00:00:00Z",
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "followups",
            &seed,
            "--path",
            "a",
            "--limit",
            "3",
            "--patch",
            "--json",
        ],
    );
    assert_eq!(report["matched_in_inspected_scope"], 5);
    assert_eq!(report["display_truncated"], true);
    assert_eq!(
        report["scope"]["order"],
        "explicit_revert_reference_then_same_file_then_forward_topological"
    );
    let entries = report["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0]["commit_id"], explicit_other_path);
    assert_eq!(entries[0]["basis"], "explicit_revert_reference");
    assert_eq!(
        entries[0]["association_bases"],
        serde_json::json!(["explicit_revert_reference"])
    );
    assert_eq!(entries[0]["revert_reference"]["target_commit_id"], seed);
    assert_eq!(entries[0]["revert_reference"]["scope"], "commit_level");
    assert_eq!(entries[0]["revert_reference"]["path_specific"], false);
    assert_eq!(entries[0]["paths"], serde_json::json!([]));
    assert_eq!(entries[0]["patch"]["status"], "available");
    assert_eq!(entries[1]["commit_id"], explicit_no_paths);
    assert_eq!(entries[2]["commit_id"], explicit_same_file);
    assert_eq!(
        entries[2]["association_bases"],
        serde_json::json!(["explicit_revert_reference", "same_file"])
    );
    assert_eq!(entries[2]["paths"], serde_json::json!(["a"]));
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry["commit_id"] == explicit_same_file)
            .count(),
        1
    );

    let complete = json(
        &repo,
        &["followups", &seed, "--path", "a", "--limit", "10", "--json"],
    );
    let complete_entries = complete["entries"].as_array().unwrap();
    let ids = complete_entries
        .iter()
        .map(|entry| entry["commit_id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![
            explicit_other_path.as_str(),
            explicit_no_paths.as_str(),
            explicit_same_file.as_str(),
            generic.as_str(),
            generic_same_file.as_str(),
        ]
    );
    assert_eq!(
        complete_entries[3]["association_bases"],
        serde_json::json!(["same_file"])
    );
    assert!(complete_entries[3].get("revert_reference").is_none());

    let text_out = repo.run(["followups", &seed, "--path", "a", "--limit", "10"]);
    assert!(text_out.status.success());
    let text = String::from_utf8(text_out.stdout).unwrap();
    assert!(text.contains("explicit_revert_reference"));
    assert!(text.contains("commit-level") && text.contains("not path-specific"));
    let text_positions = ids
        .iter()
        .map(|id| text.find(&format!("  {id}  ")).unwrap())
        .collect::<Vec<_>>();
    assert!(
        text_positions.windows(2).all(|pair| pair[0] < pair[1]),
        "{text}\n{ids:?}\n{text_positions:?}"
    );
}

#[test]
fn followups_revert_references_obey_endpoint_time_and_traversal_bounds() {
    let repo = TestRepo::new();
    let seed = commit(&repo, "a", "seed\n", "Seed", "2020-01-01T00:00:00Z");
    commit(
        &repo,
        "other",
        "nonmatch\n",
        "Nonmatch before references",
        "2020-01-02T00:00:00Z",
    );
    let inverted = commit(
        &repo,
        "other",
        "inverted\n",
        &format!("Revert\n\nThis reverts commit {seed}."),
        "2019-12-31T00:00:00Z",
    );
    let boundary = commit(
        &repo,
        "other",
        "boundary\n",
        &format!("Revert at boundary\n\nThis reverts commit {seed}."),
        "2020-01-02T00:00:00Z",
    );
    commit(
        &repo,
        "other",
        "outside\n",
        &format!("Outside window\n\nThis reverts commit {seed}."),
        "2020-01-02T00:00:01Z",
    );
    let after_endpoint = commit(
        &repo,
        "other",
        "after endpoint\n",
        &format!("After endpoint\n\nThis reverts commit {seed}."),
        "2020-01-02T00:00:00Z",
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "followups",
            &seed,
            "--days",
            "1",
            "--max-commits",
            "10",
            "--json",
        ],
    );
    let entries = report["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0]["commit_id"], inverted);
    assert_eq!(entries[0]["elapsed_seconds"], -86400);
    assert_eq!(entries[1]["commit_id"], boundary);
    assert_eq!(entries[1]["elapsed_seconds"], 86400);
    assert_eq!(entries[2]["commit_id"], after_endpoint);
    assert_eq!(report["lineage_inspected_count"], 1);
    assert!(
        report["warnings"]
            .to_string()
            .contains("Timestamp inversion")
    );

    let endpoint_report = json(
        &repo,
        &[
            "followups",
            &seed,
            "--to-rev",
            &boundary,
            "--days",
            "1",
            "--json",
        ],
    );
    let endpoint_entries = endpoint_report["entries"].as_array().unwrap();
    assert_eq!(endpoint_entries.len(), 2);
    assert_eq!(endpoint_entries[0]["commit_id"], inverted);
    assert_eq!(endpoint_entries[1]["commit_id"], boundary);

    let budget_report = json(
        &repo,
        &[
            "followups",
            &seed,
            "--days",
            "1",
            "--max-commits",
            "1",
            "--json",
        ],
    );
    assert_eq!(budget_report["inspected_count"], 1);
    assert_eq!(budget_report["traversal_truncated"], true);
    assert!(budget_report["entries"].as_array().unwrap().is_empty());
}

#[test]
fn followups_resolves_unique_abbreviated_revert_reference() {
    let repo = TestRepo::new();
    let seed = commit(&repo, "a", "seed\n", "Seed", "2020-01-01T00:00:00Z");
    let abbreviated_seed = &seed[..12];
    let reference = commit(
        &repo,
        "other",
        "unselected path\n",
        &format!("Revert note\n\nThis reverts commit {abbreviated_seed}."),
        "2020-01-02T00:00:00Z",
    );
    repo.index();

    let report = json(&repo, &["followups", &seed, "--path", "a", "--json"]);
    let entries = report["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["commit_id"], reference);
    assert_eq!(entries[0]["revert_reference"]["target_commit_id"], seed);
    assert_eq!(entries[0]["paths"], serde_json::json!([]));
}
