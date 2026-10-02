mod support;

use serde_json::Value;
use std::fs;
use support::{TestRepo, git};

fn commit(repo: &TestRepo, path: &str, text: &str) {
    fs::write(repo.dir.path().join(path), text).unwrap();
    git(repo.dir.path(), ["add", "--all"]);
    git(repo.dir.path(), ["commit", "-m", "touch"]);
}
fn report(repo: &TestRepo, args: &[&str]) -> Value {
    let output = repo.run(args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn ranks_repeated_touches_at_published_tip_and_limits_after_sorting() {
    let repo = TestRepo::new();
    commit(&repo, "often", "one\n");
    commit(&repo, "once", &"large rewrite\n".repeat(100));
    commit(&repo, "often", "two\n");
    repo.index();
    let target = repo.head();
    commit(&repo, "unindexed", "ignored\n");
    fs::remove_file(repo.dir.path().join("often")).unwrap();
    let json = report(&repo, &["hotspots", "--json"]);
    assert_eq!(json["scope"]["target_rev"], target);
    assert_eq!(json["files"].as_array().unwrap().len(), 2);
    assert_eq!(json["files"][0]["path"], "often");
    assert_eq!(json["files"][0]["touching_commits"], 2);
    assert_eq!(json["files"][1]["touching_commits"], 1);
    assert_eq!(
        report(&repo, &["hotspots", "--json", "--limit", "1"])["files"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let text = repo.run(["hotspots"]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.find("often").unwrap() < text.find("once").unwrap());
    assert!(text.contains(json["files"][0]["last_changed"].as_str().unwrap()));
    let stamp = json["files"][0]["last_changed"].as_str().unwrap();
    assert_eq!(stamp.len(), 20);
    assert!(stamp.ends_with('Z'));
    assert!(
        !json["files"][0]
            .as_object()
            .unwrap()
            .contains_key("additions")
    );
}

fn record(repo: &TestRepo) {
    git(repo.dir.path(), ["add", "--all"]);
    git(repo.dir.path(), ["commit", "-m", "record"]);
}
fn count(json: &Value, path: &str) -> u64 {
    json["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == path)
        .unwrap()["touching_commits"]
        .as_u64()
        .unwrap()
}
#[test]
fn follows_rename_chains_but_not_copies_or_recreated_paths() {
    let repo = TestRepo::new();
    commit(&repo, "old", "one\ntwo\nthree\n");
    commit(&repo, "old", "one\ntwo\nthree\nfour\n");
    git(repo.dir.path(), ["mv", "old", "middle"]);
    record(&repo);
    git(repo.dir.path(), ["mv", "middle", "new"]);
    record(&repo);
    fs::copy(repo.dir.path().join("new"), repo.dir.path().join("copy")).unwrap();
    record(&repo);
    commit(&repo, "reused", "ancient\n");
    commit(&repo, "reused", "ancient edits\n");
    git(repo.dir.path(), ["rm", "reused"]);
    record(&repo);
    commit(&repo, "reused", "unrelated\n");
    commit(&repo, "removed", "gone\n");
    git(repo.dir.path(), ["rm", "removed"]);
    record(&repo);
    repo.index();
    let json = report(&repo, &["hotspots", "--json"]);
    assert_eq!(count(&json, "new"), 4);
    assert_eq!(count(&json, "copy"), 1);
    assert_eq!(count(&json, "reused"), 1);
    assert_eq!(json["files"].as_array().unwrap().len(), 3);
}
#[test]
fn retains_branch_touches_and_rename_lineage_without_merge_contributions() {
    let repo = TestRepo::new();
    commit(&repo, "old", "one\ntwo\nthree\n");
    git(repo.dir.path(), ["checkout", "-b", "side"]);
    commit(&repo, "old", "one\ntwo\nthree\nbranch\n");
    git(repo.dir.path(), ["mv", "old", "new"]);
    record(&repo);
    git(repo.dir.path(), ["checkout", "main"]);
    commit(&repo, "main-only", "main\n");
    git(repo.dir.path(), ["merge", "--no-ff", "--no-commit", "side"]);
    fs::write(repo.dir.path().join("new"), "merge resolution\n").unwrap();
    fs::write(repo.dir.path().join("merge-only"), "merge\n").unwrap();
    record(&repo);
    repo.index();
    let json = report(&repo, &["hotspots", "--json"]);
    assert_eq!(count(&json, "new"), 3);
    assert_eq!(count(&json, "main-only"), 1);
    assert_eq!(json["files"].as_array().unwrap().len(), 2);
    assert!(
        json["policy"]["merge"]
            .as_str()
            .unwrap()
            .contains("merge-only")
    );
}
#[test]
fn includes_binary_permission_generated_and_lockfile_touches_and_maximum_time() {
    let repo = TestRepo::new();
    commit(&repo, "binary", "a\0b");
    commit(&repo, "binary", "c\0d");
    commit(&repo, "generated.lock", "generated\n");
    git(
        repo.dir.path(),
        ["update-index", "--chmod=+x", "generated.lock"],
    );
    git(repo.dir.path(), ["commit", "-m", "permissions"]);
    let output = support::git_command(repo.dir.path())
        .args(["commit", "--allow-empty", "-m", "empty"])
        .env("GIT_COMMITTER_DATE", "2030-01-01T00:00:00Z")
        .output()
        .unwrap();
    assert!(output.status.success());
    fs::write(repo.dir.path().join("dated"), "old\n").unwrap();
    git(repo.dir.path(), ["add", "dated"]);
    let output = support::git_command(repo.dir.path())
        .args(["commit", "-m", "newer time"])
        .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00Z")
        .output()
        .unwrap();
    assert!(output.status.success());
    fs::write(repo.dir.path().join("dated"), "new\n").unwrap();
    git(repo.dir.path(), ["add", "dated"]);
    let output = support::git_command(repo.dir.path())
        .args(["commit", "-m", "older time"])
        .env("GIT_COMMITTER_DATE", "2010-01-01T00:00:00Z")
        .output()
        .unwrap();
    assert!(output.status.success());
    repo.index();
    let json = report(&repo, &["hotspots", "--json"]);
    assert_eq!(count(&json, "binary"), 2);
    assert_eq!(count(&json, "generated.lock"), 2);
    let files = json["files"].as_array().unwrap();
    assert_eq!(
        files
            .iter()
            .map(|f| f["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["binary", "dated", "generated.lock"]
    );
    assert_eq!(files[1]["last_changed"], "2020-01-01T00:00:00Z");
}
#[test]
fn empty_results_default_top_twenty_and_validation() {
    let repo = TestRepo::new();
    commit(&repo, "gone", "gone\n");
    git(repo.dir.path(), ["rm", "gone"]);
    record(&repo);
    repo.index();
    assert!(
        report(&repo, &["hotspots", "--json"])["files"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(repo.run(["hotspots"]).status.success());
    for index in 0..22 {
        fs::write(repo.dir.path().join(format!("file-{index:02}")), "one\n").unwrap();
    }
    record(&repo);
    repo.index();
    let json = report(&repo, &["hotspots", "--json"]);
    assert_eq!(json["files"].as_array().unwrap().len(), 20);
    assert_eq!(json["files"][0]["path"], "file-00");
    assert_eq!(json["files"][19]["path"], "file-19");
    for args in [
        ["hotspots", "--limit", "0"],
        ["hotspots", "--limit", "-1"],
        ["hotspots", "--since", "2020-01-01"],
    ] {
        assert_eq!(repo.run(args).status.code(), Some(2));
    }
}

#[test]
fn cache_errors_and_shallow_coverage_remain_visible() {
    let repo = TestRepo::new();
    commit(&repo, "file", "one\n");
    assert_eq!(repo.run(["hotspots"]).status.code(), Some(1));
    commit(&repo, "file", "two\n");
    commit(&repo, "file", "three\n");
    let clone = tempfile::tempdir().unwrap();
    let url = format!(
        "file:///{}",
        repo.dir.path().to_string_lossy().replace('\\', "/")
    );
    git(
        repo.dir.path(),
        ["clone", "--depth=2", &url, clone.path().to_str().unwrap()],
    );
    let index = TestRepo::run_at(clone.path(), ["index"]);
    assert!(
        index.status.success(),
        "{}",
        String::from_utf8_lossy(&index.stderr)
    );
    let output = TestRepo::run_at(clone.path(), ["hotspots", "--json"]);
    assert!(output.status.success());
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(count(&json, "file"), 1);
    assert!(!json["coverage"]["warnings"].as_array().unwrap().is_empty());
    assert!(json["coverage"]["warnings"].to_string().contains("shallow"));
    let help = repo.run(["hotspots", "--help"]);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("merge-only"));
    assert!(!help.contains("--since"));
}
