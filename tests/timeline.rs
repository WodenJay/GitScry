mod support;

use std::{fs, path::Path};

use rusqlite::Connection;
use serde_json::Value;
use support::{TestRepo, git, git_command, git_stdout};

fn commit(repo: &TestRepo, path: &str, contents: &[u8], subject: &str, date: &str) -> String {
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
    fs::create_dir_all(repo.dir.path().join(new).parent().expect("new path parent"))
        .expect("create rename parent");
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
        "git commit failed: {}",
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
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .expect("run git delete commit");
    assert!(
        output.status.success(),
        "git delete failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    repo.head()
}

fn json(output: &std::process::Output) -> Value {
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("valid timeline JSON")
}

#[test]
fn timeline_json_pages_topologically_and_follows_renames() {
    let repo = TestRepo::new();
    let added = commit(
        &repo,
        "src/old.rs",
        b"first\n",
        "Add source file",
        "2020-01-01T00:00:00+0000",
    );
    let modified = commit(
        &repo,
        "src/old.rs",
        b"second\n",
        "Modify source file",
        "2019-01-01T00:00:00+0000",
    );
    let renamed = rename(
        &repo,
        "src/old.rs",
        "src/new.rs",
        "Rename source file",
        "2021-01-01T00:00:00+0000",
    );
    let changed = commit(
        &repo,
        "src/new.rs",
        b"third\n",
        "Update renamed file",
        "2022-01-01T00:00:00+0000",
    );
    repo.index();

    let first_page = json(&repo.run(["timeline", "src/new.rs", "--limit", "2", "--json"]));
    assert_eq!(first_page["kind"], "timeline");
    assert_eq!(first_page["total"], 4);
    assert_eq!(first_page["offset"], 0);
    assert_eq!(first_page["start"], 1);
    assert_eq!(first_page["end"], 2);
    assert_eq!(first_page["has_more"], true);
    assert_eq!(first_page["entries"][0]["commit_id"], added);
    assert_eq!(first_page["entries"][0]["change_type"], "added");
    assert_eq!(first_page["entries"][0]["path"], "src/old.rs");
    assert_eq!(first_page["entries"][1]["commit_id"], modified);
    assert_eq!(
        first_page["entries"][1]["timestamp"],
        "2019-01-01T00:00:00Z"
    );

    let last_page = json(&repo.run(["timeline", "src/new.rs", "--limit", "2", "--last", "--json"]));
    assert_eq!(last_page["offset"], 2);
    assert_eq!(last_page["start"], 3);
    assert_eq!(last_page["end"], 4);
    assert_eq!(last_page["has_more"], false);
    assert_eq!(last_page["entries"][0]["commit_id"], renamed);
    assert_eq!(last_page["entries"][0]["change_type"], "renamed");
    assert_eq!(last_page["entries"][0]["path"], "src/new.rs");
    assert_eq!(last_page["entries"][1]["commit_id"], changed);

    let patched = json(&repo.run(["timeline", "src/new.rs", "--patch", "--json"]));
    assert_eq!(patched["schema_version"], 2);
    assert_eq!(patched["total"], 4);
    assert_eq!(
        patched["entries"][0]["patch"]["hunks"][0]["new_path"],
        "src/old.rs"
    );
    assert_eq!(
        patched["entries"][1]["patch"]["hunks"][0]["old_path"],
        "src/old.rs"
    );
    assert_eq!(patched["entries"][2]["patch"]["status"], "unavailable");
    assert_eq!(
        patched["entries"][3]["patch"]["hunks"][0]["new_path"],
        "src/new.rs"
    );
    let old_path_at_rename = json(&repo.run([
        "timeline",
        "src/old.rs",
        "--at",
        modified.as_str(),
        "--json",
    ]));
    assert_eq!(old_path_at_rename["target_revision"], modified);
    assert_eq!(old_path_at_rename["total"], 2);
    assert_eq!(old_path_at_rename["entries"][1]["commit_id"], modified);

    let empty_page = json(&repo.run(["timeline", "src/new.rs", "--offset", "99", "--json"]));
    assert_eq!(empty_page["total"], 4);
    assert_eq!(empty_page["start"], 0);
    assert_eq!(empty_page["end"], 0);
    assert_eq!(empty_page["entries"].as_array().unwrap().len(), 0);
    assert_eq!(empty_page["has_more"], false);

    let text = repo.run(["timeline", "src/new.rs"]);
    assert_eq!(text.status.code(), Some(0));
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(text.contains(&added));
    assert!(text.contains(&format!("git show {added}")));
    assert!(text.contains("Add source file"));
    assert!(text.contains("Showing 1-4 of 4 matching commits"));

    let moved_path = repo.run(["timeline", "src/old.rs", "--json"]);
    assert!(!moved_path.status.success());
    assert!(
        String::from_utf8_lossy(&moved_path.stderr).contains("path does not exist at revision")
    );

    let help = repo.run(["timeline", "--help"]);
    assert_eq!(help.status.code(), Some(0));
    let help = String::from_utf8_lossy(&help.stdout);
    for option in [
        "--at REV",
        "--limit N",
        "--offset N",
        "--last",
        "--patch",
        "--json",
        "--github-links",
        "--github-repo <OWNER/REPO>",
    ] {
        assert!(help.contains(option), "timeline help is missing {option}");
    }
    assert!(
        help.contains("does not retrieve uncached history or commits outside the returned page")
    );
    let root_help = repo.run(["--help"]);
    let root_help_text = String::from_utf8_lossy(&root_help.stdout);
    assert!(root_help_text.contains("gitscry <command> --help"));

    let unindexed = commit(
        &repo,
        "src/new.rs",
        b"unindexed\n",
        "Unindexed change",
        "2023-01-01T00:00:00+0000",
    );
    let scoped_target = json(&repo.run([
        "timeline",
        "src/new.rs",
        "--at",
        unindexed.as_str(),
        "--to-rev",
        added.as_str(),
        "--json",
    ]));
    assert_eq!(scoped_target["target_revision"], unindexed);
    assert_eq!(scoped_target["scope"]["to_rev"], added);
    assert_eq!(scoped_target["scope"]["cache_tip"], unindexed);
    assert_eq!(scoped_target["scope"]["coverage_complete"], true);
    assert!(
        scoped_target["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["commit_id"] != unindexed)
    );

    let default_target = json(&repo.run(["timeline", "src/new.rs", "--json"]));
    assert_eq!(default_target["target_revision"], unindexed);
    assert_eq!(default_target["scope"]["to_rev"], unindexed);
    assert_eq!(default_target["scope"]["coverage_complete"], true);
    assert_eq!(default_target["total"], 5);
    assert!(
        default_target["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["commit_id"] == unindexed)
    );

    let explicit_target = json(&repo.run([
        "timeline",
        "src/new.rs",
        "--at",
        unindexed.as_str(),
        "--json",
    ]));
    assert_eq!(explicit_target["target_revision"], unindexed);
    assert_eq!(explicit_target["total"], 5);

    let conflicting_page = repo.run(["timeline", "src/new.rs", "--last", "--offset", "0"]);
    assert_eq!(conflicting_page.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&conflicting_page.stderr).contains("cannot be used with"));
}

#[test]
fn recreated_path_starts_a_new_file_incarnation() {
    let repo = TestRepo::new();
    let original = commit(
        &repo,
        "src/file.rs",
        b"original\n",
        "Original file",
        "2020-01-01T00:00:00+0000",
    );
    let original_change = commit(
        &repo,
        "src/file.rs",
        b"original changed\n",
        "Change original file",
        "2020-01-02T00:00:00+0000",
    );
    let deleted = delete(
        &repo,
        "src/file.rs",
        "Delete original file",
        "2020-01-03T00:00:00+0000",
    );
    let recreated = commit(
        &repo,
        "src/file.rs",
        b"new incarnation\n",
        "Recreate file",
        "2020-01-04T00:00:00+0000",
    );
    let final_change = commit(
        &repo,
        "src/file.rs",
        b"new incarnation changed\n",
        "Change recreated file",
        "2020-01-05T00:00:00+0000",
    );
    repo.index();

    let report = json(&repo.run(["timeline", "src/file.rs", "--json"]));
    assert_eq!(report["total"], 2);
    assert_eq!(report["entries"][0]["commit_id"], recreated);
    assert_eq!(report["entries"][0]["change_type"], "added");
    assert_eq!(report["entries"][1]["commit_id"], final_change);
    assert_ne!(report["entries"][0]["commit_id"], original);
    assert_ne!(report["entries"][0]["commit_id"], original_change);
    assert_ne!(report["entries"][0]["commit_id"], deleted);
    let patched = json(&repo.run(["timeline", "src/file.rs", "--patch", "--json"]));
    assert_eq!(patched["schema_version"], 2);
    assert_eq!(patched["total"], 2);
    assert_eq!(patched["entries"][0]["commit_id"], recreated);
    assert_eq!(patched["entries"][1]["commit_id"], final_change);
    for entry in patched["entries"].as_array().unwrap() {
        let patch = &entry["patch"];
        assert_eq!(patch["commit_oid"], entry["commit_id"]);
        assert_eq!(patch["status"], "available");
        let text = patch["hunks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|hunk| hunk["text"].as_str().unwrap())
            .collect::<String>();
        assert!(text.contains("new incarnation"));
        assert!(!text.contains("original"));
    }
}

#[test]
fn timeline_includes_merged_branch_commits_and_marks_merge_comparison() {
    let repo = TestRepo::new();
    commit(
        &repo,
        "src/file.rs",
        b"base\n",
        "Add file",
        "2020-01-01T00:00:00+0000",
    );
    git(repo.dir.path(), ["switch", "-c", "feature"]);
    let feature = commit(
        &repo,
        "src/file.rs",
        b"branch change\n",
        "Feature change",
        "2020-01-02T00:00:00+0000",
    );
    git(repo.dir.path(), ["switch", "main"]);
    commit(
        &repo,
        "README.md",
        b"unrelated\n",
        "Unrelated main change",
        "2020-01-03T00:00:00+0000",
    );
    let merge_output = git_command(repo.dir.path())
        .args(["merge", "--no-ff", "--no-edit", "feature"])
        .env("GIT_AUTHOR_DATE", "2020-01-04T00:00:00+0000")
        .env("GIT_COMMITTER_DATE", "2020-01-04T00:00:00+0000")
        .output()
        .expect("run merge commit");
    assert!(
        merge_output.status.success(),
        "merge failed: {}",
        String::from_utf8_lossy(&merge_output.stderr)
    );
    let merge = repo.head();
    repo.index();

    let report = json(&repo.run(["timeline", "src/file.rs", "--json"]));
    assert_eq!(report["total"], 3);
    assert_eq!(report["entries"][1]["commit_id"], feature);
    assert_eq!(report["entries"][2]["commit_id"], merge);
    assert_eq!(report["entries"][2]["parent_count"], 2);
    let patched = json(&repo.run(["timeline", "src/file.rs", "--patch", "--json"]));
    assert_eq!(patched["schema_version"], 2);
    assert_eq!(patched["entries"][2]["commit_id"], merge);
    assert_eq!(patched["entries"][2]["diff_comparison"], "first_parent");
    assert_eq!(patched["entries"][2]["patch"]["status"], "available");
    assert_eq!(report["entries"][2]["diff_comparison"], "first_parent");
    let text = repo.run(["timeline", "src/file.rs", "--patch"]);
    assert!(String::from_utf8_lossy(&text.stdout).contains("compared with the first parent"));
}

#[test]
fn merge_addition_does_not_reset_the_existing_file_incarnation() {
    let repo = TestRepo::new();
    commit(
        &repo,
        "README.md",
        b"base\n",
        "Create mainline",
        "2020-01-01T00:00:00+0000",
    );
    git(repo.dir.path(), ["switch", "-c", "feature"]);
    let added = commit(
        &repo,
        "src/file.rs",
        b"feature\n",
        "Add file on feature branch",
        "2020-01-02T00:00:00+0000",
    );
    let modified = commit(
        &repo,
        "src/file.rs",
        b"feature changed\n",
        "Modify file on feature branch",
        "2020-01-03T00:00:00+0000",
    );
    git(repo.dir.path(), ["switch", "main"]);
    commit(
        &repo,
        "main.txt",
        b"mainline\n",
        "Change mainline",
        "2020-01-04T00:00:00+0000",
    );
    let merge = git_command(repo.dir.path())
        .args(["merge", "--no-ff", "--no-edit", "feature"])
        .env("GIT_AUTHOR_DATE", "2020-01-05T00:00:00+0000")
        .env("GIT_COMMITTER_DATE", "2020-01-05T00:00:00+0000")
        .output()
        .expect("run merge commit");
    assert!(
        merge.status.success(),
        "merge failed: {}",
        String::from_utf8_lossy(&merge.stderr)
    );
    let merge = repo.head();
    repo.index();

    let report = json(&repo.run(["timeline", "src/file.rs", "--json"]));
    assert_eq!(report["total"], 3);
    assert_eq!(report["entries"][0]["commit_id"], added);
    assert_eq!(report["entries"][1]["commit_id"], modified);
    assert_eq!(report["entries"][2]["commit_id"], merge);
    assert_eq!(report["entries"][0]["change_type"], "added");
    assert_eq!(report["entries"][2]["change_type"], "added");
    assert_eq!(report["entries"][2]["diff_comparison"], "first_parent");
}
#[test]
fn copy_does_not_inherit_source_file_history() {
    let repo = TestRepo::new();
    let source = commit(
        &repo,
        "src/source.rs",
        b"source\n",
        "Add source",
        "2020-01-01T00:00:00+0000",
    );
    commit(
        &repo,
        "src/source.rs",
        b"source changed\n",
        "Modify source",
        "2020-01-02T00:00:00+0000",
    );
    fs::copy(
        repo.dir.path().join("src/source.rs"),
        repo.dir.path().join("src/copy.rs"),
    )
    .expect("copy file");
    git(repo.dir.path(), ["add", "--all"]);
    let output = git_command(repo.dir.path())
        .args(["commit", "-m", "Copy source"])
        .env("GIT_AUTHOR_DATE", "2020-01-03T00:00:00+0000")
        .env("GIT_COMMITTER_DATE", "2020-01-03T00:00:00+0000")
        .output()
        .expect("commit copied file");
    assert!(output.status.success());
    let copied = repo.head();
    repo.index();

    let report = json(&repo.run(["timeline", "src/copy.rs", "--json"]));
    assert_eq!(report["total"], 1);
    assert_eq!(report["entries"][0]["commit_id"], copied);
    assert_eq!(report["entries"][0]["change_type"], "added");
    assert_ne!(report["entries"][0]["commit_id"], source);
}

#[test]
fn shallow_boundary_warns_and_does_not_claim_introduction() {
    let repo = TestRepo::new();
    commit(
        &repo,
        "src/file.rs",
        b"first\n",
        "First file change",
        "2020-01-01T00:00:00+0000",
    );
    let boundary = commit(
        &repo,
        "src/file.rs",
        b"second\n",
        "Change at shallow boundary",
        "2020-01-02T00:00:00+0000",
    );
    commit(
        &repo,
        "src/file.rs",
        b"third\n",
        "Change after boundary",
        "2020-01-03T00:00:00+0000",
    );
    let head = repo.head();
    let shallow_path = git_stdout(repo.dir.path(), ["rev-parse", "--git-path", "shallow"]);
    let shallow_path = Path::new(&shallow_path);
    let shallow_path = if shallow_path.is_absolute() {
        shallow_path.to_path_buf()
    } else {
        repo.dir.path().join(shallow_path)
    };
    fs::write(shallow_path, format!("{boundary}\n")).expect("write shallow boundary");
    repo.index();

    let report = json(&repo.run(["timeline", "src/file.rs", "--json"]));
    let warnings = report["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("shallow"))
    );
    assert_eq!(report["total"], 1, "{report}");
    assert_eq!(report["entries"][0]["commit_id"], head);
    assert_eq!(report["entries"][0]["change_type"], "modified");
}

#[test]
fn timeline_patch_is_opt_in_scoped_and_preserves_pagination() {
    let repo = TestRepo::new();
    let before = (0..40)
        .map(|index| format!("let value_{index} = {index};\n"))
        .collect::<String>();
    let added = commit(
        &repo,
        "src/target.rs",
        before.as_bytes(),
        "Add target source",
        "2020-01-01T00:00:00+0000",
    );
    fs::write(
        repo.dir.path().join("src/other.rs"),
        b"let sibling = false;\n",
    )
    .expect("write sibling file");
    let after = before
        .replacen("let value_0 = 0;", "let selected_first = true;", 1)
        .replacen("let value_39 = 39;", "let selected_last = true;", 1);
    let changed = commit(
        &repo,
        "src/target.rs",
        after.as_bytes(),
        "Update target and another file",
        "2020-01-02T00:00:00+0000",
    );
    repo.index();

    let plain = json(&repo.run(["timeline", "src/target.rs", "--json"]));
    assert_eq!(plain["schema_version"], 1);
    assert_eq!(plain["total"], 2);
    assert!(plain["entries"][1].get("patch").is_none());

    let patched = json(&repo.run(["timeline", "src/target.rs", "--patch", "--json"]));
    assert_eq!(patched["schema_version"], 2);
    assert_eq!(patched["total"], plain["total"]);
    assert_eq!(patched["offset"], plain["offset"]);
    assert_eq!(patched["start"], plain["start"]);
    assert_eq!(patched["end"], plain["end"]);
    assert_eq!(patched["has_more"], plain["has_more"]);
    assert_eq!(patched["entries"][0]["commit_id"], added);
    assert_eq!(patched["entries"][1]["commit_id"], changed);

    assert_eq!(patched["entries"][0]["patch"]["status"], "available");
    assert_eq!(patched["entries"][0]["patch"]["commit_oid"], added);
    let added_text = patched["entries"][0]["patch"]["hunks"][0]["text"]
        .as_str()
        .unwrap();
    assert!(added_text.contains("value_0"));
    assert!(!added_text.contains("other.rs"));
    let patch = &patched["entries"][1]["patch"];
    assert_eq!(patch["status"], "available");
    assert_eq!(patch["commit_oid"], changed);
    let hunks = patch["hunks"].as_array().unwrap();
    assert_eq!(hunks.len(), 2);
    assert!(hunks.iter().all(|hunk| {
        hunk["new_path"] == "src/target.rs" && hunk["old_path"] == "src/target.rs"
    }));
    let text = hunks
        .iter()
        .map(|hunk| hunk["text"].as_str().unwrap())
        .collect::<String>();
    assert!(text.contains("selected_first"));
    assert!(text.contains("selected_last"));
    assert!(!text.contains("sibling"));

    let page = json(&repo.run([
        "timeline",
        "src/target.rs",
        "--limit",
        "1",
        "--offset",
        "1",
        "--patch",
        "--json",
    ]));
    assert_eq!(page["total"], 2);
    assert_eq!(page["start"], 2);
    assert_eq!(page["entries"].as_array().unwrap().len(), 1);
    assert_eq!(page["entries"][0]["commit_id"], changed);
    assert_eq!(page["entries"][0]["patch"], patch.clone());

    let first_page = json(&repo.run([
        "timeline",
        "src/target.rs",
        "--limit",
        "1",
        "--offset",
        "0",
        "--patch",
        "--json",
    ]));
    assert_eq!(first_page["start"], 1);
    assert_eq!(first_page["has_more"], true);
    let paged_ids = [
        first_page["entries"][0]["commit_id"].clone(),
        page["entries"][0]["commit_id"].clone(),
    ];
    let complete_ids = patched["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["commit_id"].clone())
        .collect::<Vec<_>>();
    assert_eq!(paged_ids.as_slice(), complete_ids.as_slice());

    let empty_page = json(&repo.run([
        "timeline",
        "src/target.rs",
        "--limit",
        "1",
        "--offset",
        "10",
        "--patch",
        "--json",
    ]));
    assert_eq!(empty_page["schema_version"], 2);
    assert_eq!(empty_page["total"], 2);
    assert_eq!(empty_page["start"], 0);
    assert_eq!(empty_page["end"], 0);
    assert_eq!(empty_page["has_more"], false);
    assert_eq!(empty_page["entries"].as_array().unwrap().len(), 0);

    let text_output = repo.run(["timeline", "src/target.rs", "--patch"]);
    assert_eq!(text_output.status.code(), Some(0));
    let text_output = String::from_utf8(text_output.stdout).expect("safe timeline output");
    assert!(text_output.contains("patch excerpt: available"));
    assert!(text_output.contains("selected_first"));
    assert!(!text_output.contains("sibling"));

    let help = repo.run(["timeline", "--help"]);
    assert!(String::from_utf8_lossy(&help.stdout).contains("--patch"));
}

#[test]
fn timeline_patch_reports_binary_text_as_unavailable() {
    let repo = TestRepo::new();
    commit(
        &repo,
        "src/data.bin",
        b"\0first binary value\n",
        "Add binary file",
        "2020-01-01T00:00:00+0000",
    );
    commit(
        &repo,
        "src/data.bin",
        b"\0second binary value\n",
        "Update binary file",
        "2020-01-02T00:00:00+0000",
    );
    repo.index();

    let report = json(&repo.run(["timeline", "src/data.bin", "--patch", "--json"]));
    assert_eq!(report["schema_version"], 2);
    assert_eq!(report["total"], 2);
    for entry in report["entries"].as_array().unwrap() {
        assert_eq!(entry["patch"]["status"], "unavailable");
        assert_eq!(entry["patch"]["hunks"], serde_json::json!([]));
    }
    let text = repo.run(["timeline", "src/data.bin", "--patch"]);
    assert!(String::from_utf8_lossy(&text.stdout).contains("Text hunk unavailable."));
}

#[test]
fn timeline_patch_binary_is_unavailable_with_text_sibling() {
    let repo = TestRepo::new();
    fs::create_dir_all(repo.dir.path().join("src")).expect("create source directory");
    fs::write(
        repo.dir.path().join("src/sibling.rs"),
        b"let sibling = true;\n",
    )
    .expect("write sibling text file");
    commit(
        &repo,
        "src/data.bin",
        b"\0binary data\n",
        "Add binary and text files",
        "2020-01-01T00:00:00+0000",
    );
    repo.index();

    let report = json(&repo.run(["timeline", "src/data.bin", "--patch", "--json"]));
    assert_eq!(report["total"], 1);
    assert_eq!(report["entries"][0]["patch"]["status"], "unavailable");
    assert_eq!(
        report["entries"][0]["patch"]["hunks"],
        serde_json::json!([])
    );
}

#[test]
fn timeline_patch_bounds_hunks_and_bytes_per_entry() {
    let repo = TestRepo::new();
    let mut before_lines = vec!["a".repeat(12 * 1024)];
    before_lines.extend((0..240).map(|index| format!("let value_{index} = false;")));
    let before = format!("{}\n", before_lines.join("\n"));
    let added = commit(
        &repo,
        "src/bounded.rs",
        before.as_bytes(),
        "Add bounded source",
        "2020-01-01T00:00:00+0000",
    );

    let mut after_lines = before_lines;
    after_lines[0] = "b".repeat(12 * 1024);
    for index in (8..after_lines.len()).step_by(12) {
        after_lines[index] = format!("let value_{index} = true;");
    }
    let after = format!("{}\n", after_lines.join("\n"));
    let changed = commit(
        &repo,
        "src/bounded.rs",
        after.as_bytes(),
        "Change bounded source",
        "2020-01-02T00:00:00+0000",
    );
    repo.index();

    let report = json(&repo.run(["timeline", "src/bounded.rs", "--patch", "--json"]));
    assert_eq!(report["entries"][0]["commit_id"], added);
    assert_eq!(report["entries"][1]["commit_id"], changed);
    let patch = &report["entries"][1]["patch"];
    let hunks = patch["hunks"].as_array().unwrap();
    assert_eq!(hunks.len(), 16);
    let first_hunk = &hunks[0];
    assert!(first_hunk["text"].as_str().unwrap().len() <= 8 * 1024);
    assert_eq!(first_hunk["truncated"], true);
    assert_eq!(patch["truncated"], true);
}

#[test]
fn timeline_scope_intersects_target_and_preserves_renames() {
    let repo = TestRepo::new();
    let base = commit(
        &repo,
        "src/old.rs",
        b"base\n",
        "Add source file",
        "2020-01-01T00:00:00+0000",
    );
    let shared = commit(
        &repo,
        "src/old.rs",
        b"shared\n",
        "Change source file",
        "2020-01-02T00:00:00+0000",
    );
    git(repo.dir.path(), ["switch", "-c", "feature"]);
    commit(
        &repo,
        "feature.txt",
        b"feature\n",
        "Add feature",
        "2020-01-03T00:00:00+0000",
    );
    let feature = repo.head();
    git(repo.dir.path(), ["switch", "main"]);
    rename(
        &repo,
        "src/old.rs",
        "src/new.rs",
        "Rename source file",
        "2020-01-04T00:00:00+0000",
    );
    let target = commit(
        &repo,
        "src/new.rs",
        b"target\n",
        "Change target source",
        "2020-01-05T00:00:00+0000",
    );
    let merge = git_command(repo.dir.path())
        .args(["merge", "--no-ff", "--no-edit", "feature"])
        .env("GIT_AUTHOR_DATE", "2020-01-06T00:00:00+0000")
        .env("GIT_COMMITTER_DATE", "2020-01-06T00:00:00+0000")
        .output()
        .expect("merge feature branch");
    assert!(merge.status.success());
    repo.index();

    let report = json(&repo.run([
        "timeline",
        "src/new.rs",
        "--at",
        target.as_str(),
        "--to-rev",
        feature.as_str(),
        "--limit",
        "1",
        "--offset",
        "1",
        "--patch",
        "--json",
    ]));
    assert_eq!(report["target_revision"], target);
    assert_eq!(report["scope"]["to_rev"], feature);
    assert_eq!(report["scope"]["target_rev"], target);
    assert_eq!(report["total"], 2);
    assert_eq!(report["start"], 2);
    assert_eq!(report["entries"][0]["commit_id"], shared);
    assert_eq!(report["entries"][0]["path"], "src/old.rs");
    assert_eq!(report["entries"][0]["patch"]["status"], "available");
    assert!(
        report["entries"][0]["patch"]["hunks"][0]["text"]
            .as_str()
            .unwrap()
            .contains("+shared")
    );

    let after_base = json(&repo.run([
        "timeline",
        "src/new.rs",
        "--at",
        target.as_str(),
        "--to-rev",
        feature.as_str(),
        "--from-rev",
        base.as_str(),
        "--json",
    ]));
    assert_eq!(after_base["total"], 1);
    assert_eq!(after_base["entries"][0]["commit_id"], shared);

    let date_scoped = json(&repo.run([
        "timeline",
        "src/new.rs",
        "--at",
        target.as_str(),
        "--to-rev",
        feature.as_str(),
        "--since",
        "2020-01-02",
        "--until",
        "2020-01-02",
        "--json",
    ]));
    assert_eq!(date_scoped["total"], 1);
    assert_eq!(date_scoped["entries"][0]["commit_id"], shared);
    assert_eq!(date_scoped["scope"]["since"], "2020-01-02");
    assert_eq!(date_scoped["scope"]["until"], "2020-01-02");

    let text = repo.run([
        "timeline",
        "src/new.rs",
        "--at",
        target.as_str(),
        "--to-rev",
        feature.as_str(),
    ]);
    assert_eq!(text.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&text.stdout).contains("intersected with target revision"));
}

#[test]
fn timeline_orders_history_after_shallow_clone_is_deepened() {
    let source = TestRepo::new();
    let oldest = commit(
        &source,
        "history.txt",
        b"oldest\n",
        "Oldest commit",
        "2020-01-01T00:00:00+0000",
    );
    let newest = commit(
        &source,
        "history.txt",
        b"newest\n",
        "Newest commit",
        "2020-01-02T00:00:00+0000",
    );

    let parent = tempfile::tempdir().unwrap();
    let clone = parent.path().join("clone");
    let cloned = git_command(parent.path())
        .args(["clone", "--depth", "1", "--no-local"])
        .arg(source.dir.path())
        .arg(&clone)
        .output()
        .unwrap();
    assert!(
        cloned.status.success(),
        "{}",
        String::from_utf8_lossy(&cloned.stderr)
    );

    let first_index = TestRepo::run_at(&clone, ["index"]);
    assert!(
        first_index.status.success(),
        "{}",
        String::from_utf8_lossy(&first_index.stderr)
    );
    git(&clone, ["fetch", "--deepen=1"]);
    assert_eq!(git_stdout(&clone, ["rev-list", "--count", "HEAD"]), "2");
    let deepened_index = TestRepo::run_at(&clone, ["index"]);
    assert!(
        deepened_index.status.success(),
        "{}",
        String::from_utf8_lossy(&deepened_index.stderr)
    );

    let cache =
        Connection::open(support::git_common_dir(&clone).join("gitscry/cache.sqlite")).unwrap();
    let newest_change_count: i64 = cache
        .query_row(
            "SELECT COUNT(*) FROM changes AS ch
             JOIN commits AS c ON c.commit_id = ch.commit_id
             WHERE c.oid = ?1",
            [&newest],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        newest_change_count, 1,
        "newest commit must be refreshed after deepening"
    );

    let timeline = json(&TestRepo::run_at(
        &clone,
        ["timeline", "history.txt", "--json"],
    ));
    assert_eq!(timeline["total"], 2, "timeline response: {timeline}");
    assert_eq!(timeline["entries"][0]["commit_id"], oldest);
    assert_eq!(timeline["entries"][1]["commit_id"], newest);
}
