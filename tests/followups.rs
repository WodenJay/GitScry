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
fn followups_reports_inspected_file_material_by_region_priority() {
    let repo = TestRepo::new();
    let seed = commit(&repo, "a", "seed\n", "Seed", "2020-01-01T00:00:00Z");
    repo.index();
    commit(
        &repo,
        "other",
        "other\n",
        "Nonmatch",
        "2020-01-02T00:00:00Z",
    );
    let early = commit(&repo, "a", "early\n", "Early", "2019-12-31T00:00:00Z");
    let later = commit(&repo, "a", "later\n", "Later", "2020-01-03T00:00:00Z");
    let report = json(&repo, &["followups", &seed, "--max-commits", "2", "--json"]);
    assert_eq!(report["kind"], "followups");
    assert_eq!(report["inspected_count"], 2);
    assert_eq!(report["traversal_truncated"], true);
    assert_eq!(report["display_truncated"], false);
    assert_eq!(report["entries"].as_array().unwrap().len(), 1);
    assert_eq!(report["entries"][0]["commit_id"], early);
    assert_eq!(report["entries"][0]["elapsed_seconds"], -86400);
    assert_eq!(report["entries"][0]["basis"], "region_overlap");
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
fn followups_tracks_changed_regions_through_shifts_and_groups_them_first() {
    let repo = TestRepo::new();
    commit(
        &repo,
        "a",
        "outside\ntarget-old\ntail\nother-old\n",
        "Baseline",
        "2020-01-01T00:00:00Z",
    );
    let seed = commit(
        &repo,
        "a",
        "outside\ntarget-seed\ntail\nother-old\n",
        "Seed",
        "2020-01-02T00:00:00Z",
    );
    let shifted = commit(
        &repo,
        "a",
        "preface\noutside\ntarget-seed\ntail\nother-old\n",
        "Shift target line",
        "2020-01-03T00:00:00Z",
    );
    let unrelated = commit(
        &repo,
        "a",
        "preface\noutside\ntarget-seed\ntail\nother-new\n",
        "Edit another region",
        "2020-01-04T00:00:00Z",
    );
    let overlap = commit(
        &repo,
        "a",
        "preface\noutside\ntarget-followup\ntail\nother-new\n",
        "Edit seed region",
        "2020-01-05T00:00:00Z",
    );
    repo.index();

    let report = json(&repo, &["followups", &seed, "--json"]);
    assert_eq!(report["schema_version"], 4);
    let entries = report["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0]["commit_id"], overlap);
    assert_eq!(entries[0]["basis"], "region_overlap");
    assert_eq!(
        entries[0]["association_bases"],
        serde_json::json!(["region_overlap", "same_file"])
    );
    assert_eq!(entries[1]["commit_id"], shifted);
    assert_eq!(entries[1]["basis"], "same_file");
    assert!(
        entries[1]["region_associations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(entries[2]["commit_id"], unrelated);
    assert_eq!(entries[2]["basis"], "same_file");
    assert!(
        entries[2]["region_associations"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let region = &entries[0]["region_associations"][0];
    assert_eq!(region["seed_revision"], seed);
    assert_eq!(region["commit_id"], overlap);
    assert_eq!(region["seed_path"], "a");
    assert_eq!(region["previous_path"], "a");
    assert_eq!(region["current_path"], "a");
    assert_eq!(
        region["seed_position"],
        serde_json::json!({"start_line": 2, "line_count": 1})
    );
    assert_eq!(
        region["previous_position"],
        serde_json::json!({"start_line": 3, "line_count": 1})
    );
    assert_eq!(
        region["current_position"],
        serde_json::json!({"start_line": 3, "line_count": 1})
    );

    let limited = json(&repo, &["followups", &seed, "--limit", "2", "--json"]);
    assert_eq!(limited["entries"].as_array().unwrap().len(), 2);
    assert_eq!(limited["entries"][0]["commit_id"], overlap);
    assert_eq!(limited["entries"][1]["commit_id"], shifted);
    assert_eq!(limited["matched_in_inspected_scope"], 3);
    assert_eq!(limited["display_truncated"], true);

    let patched = json(&repo, &["followups", &seed, "--patch", "--json"]);
    assert_eq!(patched["entries"][0]["patch"]["status"], "available");
    let text = String::from_utf8(repo.run(["followups", &seed]).stdout).unwrap();
    assert!(text.contains("region_overlap"));
    assert!(text.contains(&format!("{seed}@a:2+1")), "{text}");
    assert!(text.contains(&format!("{overlap}@a:3+1")));
}

#[test]
fn followups_does_not_overlap_insertions_at_deleted_region_seams() {
    let repo = TestRepo::new();
    commit(
        &repo,
        "a",
        "prefix\nsuffix\n",
        "Baseline",
        "2020-01-01T00:00:00Z",
    );
    let seed = commit(
        &repo,
        "a",
        "prefix\ntracked one\ntracked two\ntracked three\nsuffix\n",
        "Seed region",
        "2020-01-02T00:00:00Z",
    );
    let deletion = commit(
        &repo,
        "a",
        "prefix\ntracked one\ntracked three\nsuffix\n",
        "Delete inside seed region",
        "2020-01-03T00:00:00Z",
    );
    let nearby = commit(
        &repo,
        "a",
        "prefix\ntracked one\nnearby insertion\ntracked three\nsuffix\n",
        "Insert at deleted seam",
        "2020-01-04T00:00:00Z",
    );
    repo.index();

    let report = json(&repo, &["followups", &seed, "--json"]);
    let entries = report["entries"].as_array().unwrap();
    let deletion_entry = entries
        .iter()
        .find(|entry| entry["commit_id"] == deletion)
        .unwrap();
    assert_eq!(deletion_entry["basis"], "region_overlap");
    let nearby_entry = entries
        .iter()
        .find(|entry| entry["commit_id"] == nearby)
        .unwrap();
    assert_eq!(nearby_entry["basis"], "same_file");
    assert!(
        nearby_entry["region_associations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn followups_downgrades_when_region_tracking_work_budget_is_exhausted() {
    let repo = TestRepo::new();
    let (mut baseline, mut seeded) = (String::new(), String::new());
    for index in 0..800 {
        baseline.push_str(&format!("o{index:x}\ns{index:x}\n"));
        seeded.push_str(&format!("n{index:x}\ns{index:x}\n"));
    }
    commit(&repo, "a", &baseline, "Baseline", "2020-01-01T00:00:00Z");
    let seed = commit(
        &repo,
        "a",
        &seeded,
        "Seed many separated regions",
        "2020-01-02T00:00:00Z",
    );
    let followup = seeded.replacen("n190\n", "z190\n", 1);
    let changed = commit(
        &repo,
        "a",
        &followup,
        "Change one region",
        "2020-01-03T00:00:00Z",
    );
    repo.index();

    let report = json(&repo, &["followups", &seed, "--json"]);
    let entry = report["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["commit_id"] == changed)
        .unwrap();
    assert_eq!(entry["basis"], "same_file");
    assert_eq!(
        entry["region_tracking_downgrades"][0]["reason"],
        "tracking_budget_exhausted"
    );

    let text = String::from_utf8(repo.run(["followups", &seed]).stdout).unwrap();
    assert!(text.contains("tracking_budget_exhausted"));
}

#[test]
fn followups_tracks_seed_regions_across_renames() {
    let repo = TestRepo::new();
    commit(
        &repo,
        "old",
        "outside\ntarget-old\ntail\n",
        "Baseline",
        "2020-01-01T00:00:00Z",
    );
    let seed = commit(
        &repo,
        "old",
        "outside\ntarget-seed\ntail\n",
        "Seed",
        "2020-01-02T00:00:00Z",
    );
    git(repo.dir.path(), ["mv", "old", "new"]);
    let rename = commit_all(&repo, "Rename tracked file", "2020-01-03T00:00:00Z");
    let followup = commit(
        &repo,
        "new",
        "outside\ntarget-followup\ntail\n",
        "Edit tracked region after rename",
        "2020-01-04T00:00:00Z",
    );
    repo.index();

    let report = json(&repo, &["followups", &seed, "--json"]);
    let entries = report["entries"].as_array().unwrap();
    let rename_entry = entries
        .iter()
        .find(|entry| entry["commit_id"] == rename)
        .unwrap();
    assert_eq!(rename_entry["basis"], "same_file");
    let followup_entry = entries
        .iter()
        .find(|entry| entry["commit_id"] == followup)
        .unwrap();
    assert_eq!(followup_entry["basis"], "region_overlap");
    let region = &followup_entry["region_associations"][0];
    assert_eq!(region["seed_path"], "old");
    assert_eq!(region["previous_path"], "new");
    assert_eq!(region["current_path"], "new");
    assert_eq!(region["seed_position"]["start_line"], 2);
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
fn followups_accepts_windows_style_path_separators() {
    let repo = TestRepo::new();
    fs::create_dir_all(repo.dir.path().join("src")).unwrap();
    let seed = commit(
        &repo,
        "src/foo.rs",
        "seed\n",
        "Seed",
        "2020-01-01T00:00:00Z",
    );
    let changed = commit(
        &repo,
        "src/foo.rs",
        "follow-up\n",
        "Follow-up",
        "2020-01-02T00:00:00Z",
    );
    repo.index();

    let windows_path = json(
        &repo,
        &["followups", &seed, "--path", r"src\foo.rs", "--json"],
    );
    let git_path = json(
        &repo,
        &["followups", &seed, "--path", "src/foo.rs", "--json"],
    );
    assert_eq!(windows_path["entries"], git_path["entries"]);
    assert_eq!(windows_path["entries"][0]["commit_id"], changed);
    assert_eq!(
        windows_path["scope"]["selected_paths"],
        serde_json::json!(["src/foo.rs"])
    );
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
    let explicit_endpoint = json(
        &repo,
        &["followups", &seed, "--to-rev", &unindexed, "--json"],
    );
    assert_eq!(explicit_endpoint["scope"]["endpoint"], unindexed);
    assert_eq!(explicit_endpoint["inspected_count"], 1);
    let help = repo.run(["followups", "--help"]);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("--patch"));
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
fn followups_tracks_multiple_renames_and_keeps_path_context() {
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
    let edited_new = commit(
        &repo,
        "new",
        "edit one\n",
        "Edit new path",
        "2020-01-03T00:00:00Z",
    );
    git(repo.dir.path(), ["mv", "new", "intermediate"]);
    let first_rename = commit_all(&repo, "First rename", "2020-01-04T00:00:00Z");
    let edited_intermediate = commit(
        &repo,
        "intermediate",
        "edit two\n",
        "Edit intermediate path",
        "2020-01-05T00:00:00Z",
    );
    git(repo.dir.path(), ["mv", "intermediate", "latest"]);
    let second_rename = commit_all(&repo, "Second rename", "2020-01-06T00:00:00Z");
    let edited_latest = commit(
        &repo,
        "latest",
        "edit three\n",
        "Edit latest path",
        "2020-01-07T00:00:00Z",
    );
    fs::copy(repo.dir.path().join("latest"), repo.dir.path().join("copy")).unwrap();
    let copied = commit_all(&repo, "Copy tracked file", "2020-01-08T00:00:00Z");
    let copied_edit = commit(
        &repo,
        "copy",
        "copy edit\n",
        "Edit copy",
        "2020-01-09T00:00:00Z",
    );
    fs::remove_file(repo.dir.path().join("latest")).unwrap();
    let deletion = commit_all(&repo, "Delete current path", "2020-01-10T00:00:00Z");
    let recreated = commit(
        &repo,
        "latest",
        "recreated\n",
        "Recreate current path",
        "2020-01-11T00:00:00Z",
    );
    let recreated_edit = commit(
        &repo,
        "latest",
        "recreated edit\n",
        "Edit recreation",
        "2020-01-12T00:00:00Z",
    );
    repo.index();

    let old = json(&repo, &["followups", &seed, "--path", "old", "--json"]);
    let new = json(&repo, &["followups", &seed, "--path", "new", "--json"]);
    let both = json(
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
    assert_eq!(old["schema_version"], 4);
    assert_eq!(old["entries"], new["entries"]);
    assert_eq!(old["entries"], both["entries"]);
    let entries = old["entries"].as_array().unwrap();
    let ids = entries
        .iter()
        .map(|entry| entry["commit_id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![
            edited_new.as_str(),
            first_rename.as_str(),
            edited_intermediate.as_str(),
            second_rename.as_str(),
            edited_latest.as_str(),
            deletion.as_str(),
        ]
    );
    assert_eq!(entries[0]["file_associations"][0]["seed_old_path"], "old");
    assert_eq!(entries[0]["file_associations"][0]["seed_new_path"], "new");
    assert_eq!(entries[1]["file_associations"][0]["previous_path"], "new");
    assert_eq!(
        entries[1]["file_associations"][0]["current_path"],
        "intermediate"
    );
    assert_eq!(
        entries[3]["file_associations"][0]["previous_path"],
        "intermediate"
    );
    assert_eq!(entries[3]["file_associations"][0]["current_path"], "latest");
    assert_eq!(
        entries[5]["file_associations"][0]["previous_path"],
        "latest"
    );
    assert!(entries[5]["file_associations"][0]["current_path"].is_null());
    assert!(!ids.contains(&copied.as_str()));
    assert!(!ids.contains(&copied_edit.as_str()));
    assert!(!ids.contains(&recreated.as_str()));
    assert!(!ids.contains(&recreated_edit.as_str()));

    let patched = json(
        &repo,
        &["followups", &seed, "--path", "old", "--patch", "--json"],
    );
    let latest_patch = patched["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["commit_id"] == edited_latest)
        .unwrap();
    assert_eq!(latest_patch["patch"]["status"], "available");

    let text = repo.run(["followups", &seed, "--path", "old"]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.contains("seed: old -> new"));
    assert!(text.contains("new -> intermediate"));
    assert!(text.contains("latest -> <deleted>"));
}

#[test]
fn followups_tracks_two_simultaneous_renames_separately() {
    let repo = TestRepo::new();
    fs::write(repo.dir.path().join("a"), "contents for A\n").unwrap();
    fs::write(repo.dir.path().join("b"), "contents for B\n").unwrap();
    let seed = commit_all(&repo, "Seed both files", "2020-01-01T00:00:00Z");
    git(repo.dir.path(), ["mv", "a", "c"]);
    git(repo.dir.path(), ["mv", "b", "d"]);
    let renames = commit_all(&repo, "Rename both paths", "2020-01-02T00:00:00Z");
    let edit_c = commit(
        &repo,
        "c",
        "edited contents for A\n",
        "Edit path C",
        "2020-01-03T00:00:00Z",
    );
    let edit_d = commit(
        &repo,
        "d",
        "edited contents for B\n",
        "Edit path D",
        "2020-01-04T00:00:00Z",
    );
    repo.index();

    let report = json(&repo, &["followups", &seed, "--json"]);
    let entries = report["entries"].as_array().unwrap();
    let association = |commit_id: &str, seed_path: &str| {
        entries
            .iter()
            .find(|entry| entry["commit_id"] == commit_id)
            .unwrap()["file_associations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|association| association["seed_new_path"] == seed_path)
            .unwrap()
    };

    let file_a = association(&renames, "a");
    assert_eq!(file_a["previous_path"], "a");
    assert_eq!(file_a["current_path"], "c");
    let file_b = association(&renames, "b");
    assert_eq!(file_b["previous_path"], "b");
    assert_eq!(file_b["current_path"], "d");
    assert_eq!(association(&edit_c, "a")["current_path"], "c");
    assert_eq!(association(&edit_d, "b")["current_path"], "d");
}

#[test]
fn followups_does_not_resurrect_seed_deleted_incarnation() {
    let repo = TestRepo::new();
    commit(&repo, "a", "original\n", "Original", "2020-01-01T00:00:00Z");
    fs::remove_file(repo.dir.path().join("a")).unwrap();
    let seed = commit_all(&repo, "Seed deletion", "2020-01-02T00:00:00Z");
    commit(
        &repo,
        "a",
        "recreated\n",
        "Recreate",
        "2020-01-03T00:00:00Z",
    );
    repo.index();

    let report = json(&repo, &["followups", &seed, "--json"]);
    assert!(report["entries"].as_array().unwrap().is_empty());
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
fn followups_warns_when_first_parent_deletes_a_surviving_incarnation() {
    let repo = TestRepo::new();
    let seed = commit(&repo, "a", "seed\n", "Seed", "2020-01-01T00:00:00Z");
    git(repo.dir.path(), ["checkout", "-b", "survivor"]);
    let survivor_edit = commit(
        &repo,
        "a",
        "survivor version\n",
        "Survivor edit",
        "2020-01-02T00:00:00Z",
    );
    git(repo.dir.path(), ["checkout", "-b", "deleted", &seed]);
    fs::remove_file(repo.dir.path().join("a")).unwrap();
    let deletion = commit_all(&repo, "Delete on first parent", "2020-01-03T00:00:00Z");
    git(
        repo.dir.path(),
        ["checkout", "-b", "merge-branch", &deletion],
    );
    let conflict = git_command(repo.dir.path())
        .args(["merge", "--no-ff", "--no-commit", "survivor"])
        .output()
        .unwrap();
    assert!(!conflict.status.success());
    git(repo.dir.path(), ["checkout", "--theirs", "--", "a"]);
    git(repo.dir.path(), ["add", "a"]);
    let merge_output = git_command(repo.dir.path())
        .args(["commit", "-m", "Merge surviving file"])
        .env("GIT_AUTHOR_DATE", "2020-01-04T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-04T00:00:00Z")
        .output()
        .unwrap();
    assert!(
        merge_output.status.success(),
        "{}",
        String::from_utf8_lossy(&merge_output.stderr)
    );
    let merge = repo.head();
    let after_merge = commit(
        &repo,
        "a",
        "after merge\n",
        "After merge edit",
        "2020-01-05T00:00:00Z",
    );
    git(repo.dir.path(), ["branch", "-f", "main", "HEAD"]);
    repo.index();

    let report = json(
        &repo,
        &["followups", &seed, "--to-rev", &after_merge, "--json"],
    );
    let entries = report["entries"].as_array().unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry["commit_id"] == survivor_edit)
    );
    assert!(!entries.iter().any(|entry| entry["commit_id"] == merge));
    assert!(
        !entries
            .iter()
            .any(|entry| entry["commit_id"] == after_merge)
    );
    assert!(
        report["warnings"]
            .to_string()
            .contains("ambiguous merge file correspondence")
    );

    let survivor_report = json(
        &repo,
        &["followups", &seed, "--to-rev", &survivor_edit, "--json"],
    );
    assert_eq!(survivor_report["entries"][0]["commit_id"], survivor_edit);
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
    let cache_path = repo.cache_dir().join("cache.sqlite");
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
        "explicit_revert_reference_then_region_overlap_then_same_file_then_patch_relationship_then_forward_topological"
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
        serde_json::json!(["explicit_revert_reference", "region_overlap", "same_file"])
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
        serde_json::json!(["region_overlap", "same_file"])
    );
    assert!(complete_entries[3].get("revert_reference").is_none());

    let text_out = repo.run(["followups", &seed, "--path", "a", "--limit", "10"]);
    assert!(text_out.status.success());
    let text = String::from_utf8(text_out.stdout).unwrap();
    assert!(text.contains("explicit_revert_reference"));
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

#[test]
fn followups_aggregates_repeated_diagnostics_by_default_and_verbose_restores_them() {
    let repo = TestRepo::new();
    let base = commit(
        &repo,
        "b",
        "base
",
        "Base",
        "2019-12-01T00:00:00Z",
    );
    git(repo.dir.path(), ["checkout", "-b", "topic"]);
    let seed = commit(
        &repo,
        "a",
        "seed
",
        "Seed",
        "2020-01-01T00:00:00Z",
    );
    // Merging the seed branch with a first parent outside the seed lineage
    // leaves the first-parent state empty; every later commit repeats the
    // first-parent correspondence diagnostic.
    git(repo.dir.path(), ["checkout", "main"]);
    let out = git_command(repo.dir.path())
        .args(["merge", "--no-ff", "topic", "-m", "Merge"])
        .env("GIT_AUTHOR_DATE", "2020-01-05T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-05T00:00:00Z")
        .output()
        .unwrap();
    assert!(out.status.success());
    let merge = repo.head();
    commit(
        &repo,
        "b",
        "after 1
",
        "After 1",
        "2019-12-30T00:00:00Z",
    );
    commit(
        &repo,
        "b",
        "after 2
",
        "After 2",
        "2019-12-31T00:00:00Z",
    );
    commit(
        &repo,
        "b",
        "out 1
",
        "Out 1",
        "2020-04-01T00:00:00Z",
    );
    commit(
        &repo,
        "b",
        "out 2
",
        "Out 2",
        "2020-04-02T00:00:00Z",
    );
    assert_ne!(merge, base);
    repo.index();

    let report = json(
        &repo,
        &[
            "followups",
            &seed,
            "--days",
            "60",
            "--max-commits",
            "10",
            "--json",
        ],
    );
    // JSON keeps full warnings regardless of presentation mode.
    let json_warnings = report["warnings"].as_array().unwrap().to_owned();
    assert!(
        json_warnings
            .iter()
            .any(|w| w.to_string().contains("Timestamp inversion at"))
    );
    assert!(
        json_warnings
            .iter()
            .any(|w| w.to_string().contains("first-parent file correspondence"))
    );

    let out = repo.run(["followups", &seed, "--days", "60", "--max-commits", "10"]);
    assert!(out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    // One summary per populated category, not one line per occurrence.
    assert_eq!(
        stderr.matches("first-parent file correspondence").count(),
        2
    );
    assert_eq!(stderr.matches("earlier than the seed time").count(), 1);
    // Counts separate eligible inspection from out-of-window lineage traversal.
    assert!(stderr.contains("3 inspected commits lack first-parent"));
    assert!(stderr.contains("2 inspected commits have commit times earlier than the seed time"));
    assert_eq!(
        stderr
            .matches("Use --verbose for per-commit diagnostics.")
            .count(),
        1
    );
    // Aggregation omits per-commit diagnostic details.
    assert!(!stderr.contains("signed elapsed"));
    assert!(!stderr.contains("file tracking stopped for"));

    let verbose = repo.run([
        "followups",
        &seed,
        "--days",
        "60",
        "--max-commits",
        "10",
        "--verbose",
    ]);
    assert!(verbose.status.success());
    let verbose_stderr = String::from_utf8(verbose.stderr).unwrap();
    // Full per-commit diagnostics in generation order, no summary block.
    assert_eq!(
        verbose_stderr
            .matches("first-parent file correspondence to the seed is unavailable")
            .count(),
        5
    );
    assert_eq!(verbose_stderr.matches("Timestamp inversion").count(), 2);
    assert!(verbose_stderr.contains("signed elapsed"));
    assert!(!verbose_stderr.contains("Use --verbose"));
    assert!(!verbose_stderr.contains("earlier than the seed time"));

    // Same query, same result: presentation only.
    let verbose_json = json(
        &repo,
        &[
            "followups",
            &seed,
            "--days",
            "60",
            "--max-commits",
            "10",
            "--verbose",
            "--json",
        ],
    );
    assert_eq!(report, verbose_json);
    assert_eq!(out.stdout, verbose.stdout);
}

#[test]
fn followups_distinguishes_file_and_region_merge_correspondence_categories() {
    let repo = TestRepo::new();
    let base = commit(
        &repo,
        "a",
        "base
",
        "Base",
        "2020-01-01T00:00:00Z",
    );
    git(repo.dir.path(), ["checkout", "-b", "side1"]);
    commit(
        &repo,
        "a",
        "side one
",
        "Side one",
        "2020-01-02T00:00:00Z",
    );
    git(repo.dir.path(), ["checkout", "-b", "seed-branch", &base]);
    let seed = commit(
        &repo,
        "a",
        "seed
",
        "Seed",
        "2020-01-03T00:00:00Z",
    );
    git(repo.dir.path(), ["checkout", "side1"]);
    let out = git_command(repo.dir.path())
        .args([
            "merge",
            "-s",
            "ours",
            "--no-ff",
            "seed-branch",
            "-m",
            "Merge",
        ])
        .env("GIT_AUTHOR_DATE", "2020-01-04T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-04T00:00:00Z")
        .output()
        .unwrap();
    assert!(out.status.success());
    // Continuation commit after the merge: the seed file exists on side1's
    // line but the merge carried no seed-line patches, so the region state
    // from the seed parent disagrees with the nonexistent one from side1.
    commit(
        &repo,
        "a",
        "continuation
",
        "Continuation",
        "2020-01-05T00:00:00Z",
    );
    git(repo.dir.path(), ["branch", "-f", "main", "HEAD"]);
    repo.index();

    let report = json(&repo, &["followups", &seed, "--json"]);
    assert!(report["warnings"].as_array().unwrap().iter().any(|w| {
        w.to_string()
            .contains("ambiguous merge file correspondence")
    }));

    // Default output aggregates per-commit diagnostics into category summaries.
    let out = repo.run(["followups", &seed]);
    assert!(
        out.status.success(),
        "followups exit {:?}: stdout={} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_eq!(
        stderr
            .matches("ambiguous merge file correspondence")
            .count(),
        1
    );
    assert!(stderr.contains("2 inspected commits lack first-parent file correspondence"));
    assert!(stderr.contains(
        "1 file-tracking instance across 1 inspected commit has ambiguous merge file correspondence"
    ));
    assert!(!stderr.contains("file tracking stopped for"));
    assert!(stderr.contains("Use --verbose for per-commit diagnostics."));

    let verbose = repo.run(["followups", &seed, "--verbose"]);
    assert!(verbose.status.success());
    let verbose_stderr = String::from_utf8(verbose.stderr).unwrap();
    assert!(verbose_stderr.contains("ambiguous merge file correspondence; file tracking stopped"));
    assert!(!verbose_stderr.contains("Use --verbose"));
}

#[test]
fn followups_discovers_patch_only_relationships_within_descendant_window() {
    let repo = TestRepo::new();
    let base = commit(&repo, "a", "seed\n", "Base", "2020-01-01T00:00:00Z");

    git(repo.dir.path(), ["checkout", "-b", "parallel", &base]);
    fs::remove_file(repo.dir.path().join("a")).unwrap();
    let parallel = commit_all(&repo, "Parallel deletion", "2020-01-02T00:00:00Z");

    git(repo.dir.path(), ["checkout", "main"]);
    fs::remove_file(repo.dir.path().join("a")).unwrap();
    let seed = commit_all(&repo, "Seed deletion", "2020-01-03T00:00:00Z");
    let merge = git_command(repo.dir.path())
        .args([
            "merge",
            "--no-ff",
            "parallel",
            "-m",
            "Merge parallel deletion",
        ])
        .env("GIT_AUTHOR_DATE", "2020-01-03T01:00:00Z")
        .env("GIT_COMMITTER_DATE", "2020-01-03T01:00:00Z")
        .output()
        .unwrap();
    assert!(
        merge.status.success(),
        "{}",
        String::from_utf8_lossy(&merge.stderr)
    );
    let inverse = commit(&repo, "a", "seed\n", "Recreate", "2020-01-04T00:00:00Z");
    fs::remove_file(repo.dir.path().join("a")).unwrap();
    let equivalent = commit_all(&repo, "Delete again", "2020-01-05T00:00:00Z");
    let outside = commit(
        &repo,
        "a",
        "seed\n",
        "Outside window",
        "2020-01-10T00:00:00Z",
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "followups",
            &seed,
            "--days",
            "2",
            "--max-commits",
            "20",
            "--json",
        ],
    );
    assert_eq!(
        report["scope"]["association"],
        "explicit_revert_reference_or_region_overlap_or_same_file_or_patch_relationship"
    );
    assert_eq!(
        report["patch_matching"]["seed_patch"]["integrity"],
        "complete"
    );
    assert_eq!(report["patch_matching"]["coverage_complete"], true);
    assert_eq!(report["patch_matching"]["checked_count"], 3);
    assert_eq!(report["patch_matching"]["unexamined_count"], 0);
    let entries = report["entries"].as_array().unwrap();
    for (commit_id, relation) in [(&inverse, "inverse"), (&equivalent, "equivalent")] {
        let entry = entries
            .iter()
            .find(|entry| entry["commit_id"] == commit_id.as_str())
            .unwrap();
        assert_eq!(entry["basis"], "patch_relationship");
        assert_eq!(
            entry["association_bases"],
            serde_json::json!(["patch_relationship"])
        );
        let relationship = &entry["patch_relationships"][0];
        assert_eq!(relationship["relation"], relation);
        assert_eq!(relationship["seed_commit_id"], seed);
        assert_eq!(relationship["commit_id"], commit_id.as_str());
        assert_eq!(relationship["seed_comparison_basis"], "first_parent");
        assert_eq!(relationship["comparison_basis"], "first_parent");
        assert_eq!(relationship["normalization_version"], 3);
        assert_eq!(relationship["paths"], serde_json::json!(["a"]));
    }
    assert!(
        entries
            .iter()
            .all(|entry| { entry["commit_id"] != parallel && entry["commit_id"] != outside })
    );

    let patched = json(
        &repo,
        &[
            "followups",
            &seed,
            "--days",
            "2",
            "--max-commits",
            "20",
            "--patch",
            "--json",
        ],
    );
    let inverse_entry = patched["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["commit_id"] == inverse.as_str())
        .unwrap();
    assert_eq!(inverse_entry["patch"]["status"], "available");

    let text = repo.run(["followups", &seed, "--days", "2", "--max-commits", "20"]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.contains(&format!("Patch relationship: inverse (seed {seed}")));
    assert!(text.contains(&format!("Patch relationship: equivalent (seed {seed}")));
}

#[test]
fn followups_keeps_declared_reverts_and_patch_inverse_as_distinct_bases() {
    let repo = TestRepo::new();
    commit(&repo, "a", "old\n", "Base", "2020-01-01T00:00:00Z");
    let seed = commit(&repo, "a", "new\n", "Seed", "2020-01-02T00:00:00Z");
    let declared_inverse = commit(
        &repo,
        "a",
        "old\n",
        &format!("Revert seed\n\nThis reverts commit {seed}."),
        "2020-01-03T00:00:00Z",
    );
    let equivalent = commit(&repo, "a", "new\n", "Reapply", "2020-01-04T00:00:00Z");
    repo.index();

    let report = json(
        &repo,
        &["followups", &seed, "--path", "a", "--days", "2", "--json"],
    );
    let entries = report["entries"].as_array().unwrap();
    let inverse_entry = entries
        .iter()
        .find(|entry| entry["commit_id"] == declared_inverse.as_str())
        .unwrap();
    assert_eq!(inverse_entry["basis"], "explicit_revert_reference");
    assert!(
        inverse_entry["association_bases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|basis| basis == "explicit_revert_reference")
    );
    assert!(
        inverse_entry["association_bases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|basis| basis == "patch_relationship")
    );
    assert_eq!(inverse_entry["revert_reference"]["target_commit_id"], seed);
    assert_eq!(
        inverse_entry["patch_relationships"][0]["relation"],
        "inverse"
    );
    let equivalent_entry = entries
        .iter()
        .find(|entry| entry["commit_id"] == equivalent.as_str())
        .unwrap();
    assert_eq!(
        equivalent_entry["patch_relationships"][0]["relation"],
        "equivalent"
    );

    let text = repo.run(["followups", &seed, "--path", "a", "--days", "2"]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.contains(&format!("Patch relationship: inverse (seed {seed}")));
    assert!(text.contains(&declared_inverse));
    assert!(text.contains("normalization v3"));
}

#[test]
fn followups_discloses_incomplete_seed_patch_without_losing_same_file_matches() {
    let repo = TestRepo::new();
    commit(&repo, "a", "base\n", "Base", "2020-01-01T00:00:00Z");
    let seed = commit(
        &repo,
        "a",
        "binary\0seed\n",
        "Binary seed",
        "2020-01-02T00:00:00Z",
    );
    let followup = commit(
        &repo,
        "a",
        "text again\n",
        "Followup",
        "2020-01-03T00:00:00Z",
    );
    repo.index();

    let report = json(&repo, &["followups", &seed, "--days", "2", "--json"]);
    assert_eq!(
        report["patch_matching"]["seed_patch"]["integrity"],
        "indeterminate"
    );
    assert_eq!(report["patch_matching"]["coverage_complete"], false);
    assert_eq!(report["patch_matching"]["checked_count"], 0);
    assert_eq!(report["patch_matching"]["unexamined_count"], 1);
    assert!(
        report["patch_matching"]["seed_patch"]["reason"]
            .as_str()
            .unwrap()
            .contains("binary or unavailable changed content")
    );
    let entry = report["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["commit_id"] == followup.as_str())
        .unwrap();
    assert_eq!(entry["basis"], "same_file");
    assert_eq!(entry["patch_relationships"], serde_json::json!([]));
}

#[test]
fn followups_marks_missing_cached_patch_objects_indeterminate_but_keeps_associations() {
    let repo = TestRepo::new();
    commit(&repo, "a", "old\n", "Base", "2020-01-01T00:00:00Z");
    let seed = commit(&repo, "a", "new\n", "Seed", "2020-01-02T00:00:00Z");
    let followup = commit(&repo, "a", "later\n", "Followup", "2020-01-03T00:00:00Z");
    repo.index();

    let first = json(&repo, &["followups", &seed, "--days", "2", "--json"]);
    assert_eq!(first["patch_matching"]["coverage_complete"], true);
    let blob = repo.head_oid("HEAD:a");
    let object_path = repo
        .common_dir()
        .join("objects")
        .join(&blob[..2])
        .join(&blob[2..]);
    fs::remove_file(object_path).unwrap();

    let report = json(&repo, &["followups", &seed, "--days", "2", "--json"]);
    assert_eq!(report["patch_matching"]["coverage_complete"], false);
    assert_eq!(
        report["patch_matching"]["indeterminate"][0]["commit_id"],
        followup
    );
    assert_eq!(
        report["patch_matching"]["indeterminate"][0]["reason"],
        "cached patch source objects are unavailable"
    );
    let entry = report["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["commit_id"] == followup.as_str())
        .unwrap();
    assert_eq!(entry["basis"], "region_overlap");
    assert!(
        entry["association_bases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|basis| basis == "same_file")
    );
    assert_eq!(entry["patch_relationships"], serde_json::json!([]));
}

#[test]
fn followups_path_filter_does_not_match_partial_seed_patches() {
    let repo = TestRepo::new();
    commit(&repo, "a", "old-a\n", "Base a", "2020-01-01T00:00:00Z");
    commit(&repo, "b", "old-b\n", "Base b", "2020-01-01T01:00:00Z");
    fs::write(repo.dir.path().join("a"), "new-a\n").unwrap();
    fs::write(repo.dir.path().join("b"), "new-b\n").unwrap();
    let seed = commit_all(&repo, "Two-file seed", "2020-01-02T00:00:00Z");

    fs::write(repo.dir.path().join("a"), "old-a\n").unwrap();
    fs::write(repo.dir.path().join("b"), "old-b\n").unwrap();
    commit_all(&repo, "Undo both files", "2020-01-03T00:00:00Z");
    fs::write(repo.dir.path().join("a"), "new-a\n").unwrap();
    let partial = commit_all(&repo, "Reapply one file", "2020-01-04T00:00:00Z");
    fs::write(repo.dir.path().join("a"), "old-a\n").unwrap();
    commit_all(&repo, "Undo one file", "2020-01-05T00:00:00Z");
    fs::write(repo.dir.path().join("a"), "new-a\n").unwrap();
    fs::write(repo.dir.path().join("b"), "new-b\n").unwrap();
    let equivalent = commit_all(&repo, "Reapply both files", "2020-01-06T00:00:00Z");
    repo.index();

    let report = json(
        &repo,
        &[
            "followups",
            &seed,
            "--path",
            "a",
            "--days",
            "10",
            "--patch",
            "--json",
        ],
    );
    assert_eq!(report["scope"]["selected_paths"], serde_json::json!(["a"]));
    let entries = report["entries"].as_array().unwrap();
    let partial_entry = entries
        .iter()
        .find(|entry| entry["commit_id"] == partial.as_str())
        .unwrap();
    assert_eq!(partial_entry["patch_relationships"], serde_json::json!([]));
    let equivalent_entry = entries
        .iter()
        .find(|entry| entry["commit_id"] == equivalent.as_str())
        .unwrap();
    assert_eq!(
        equivalent_entry["patch_relationships"][0]["relation"],
        "equivalent"
    );
    assert_eq!(
        equivalent_entry["patch_relationships"][0]["paths"],
        serde_json::json!(["a", "b"])
    );
    assert_eq!(equivalent_entry["patch"]["status"], "available");
}
