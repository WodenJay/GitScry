mod support;

use std::{fs, path::Path};

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
    for option in ["--at REV", "--limit N", "--offset N", "--last", "--json"] {
        assert!(help.contains(option), "timeline help is missing {option}");
    }
    let root_help = repo.run(["--help"]);
    assert!(String::from_utf8_lossy(&root_help.stdout).contains("gitscry timeline PATH"));

    let unindexed = commit(
        &repo,
        "src/new.rs",
        b"unindexed\n",
        "Unindexed change",
        "2023-01-01T00:00:00+0000",
    );
    let default_target = json(&repo.run(["timeline", "src/new.rs", "--json"]));
    assert_eq!(default_target["target_revision"], changed);
    assert_eq!(default_target["total"], 4);
    assert!(
        !default_target["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["commit_id"] == unindexed)
    );

    let outside_cache = repo.run(["timeline", "src/new.rs", "--at", unindexed.as_str()]);
    assert!(!outside_cache.status.success());
    assert!(
        String::from_utf8_lossy(&outside_cache.stderr)
            .contains("outside the published cache generation")
    );

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
    assert_eq!(report["entries"][2]["diff_comparison"], "first_parent");
    let text = repo.run(["timeline", "src/file.rs"]);
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
