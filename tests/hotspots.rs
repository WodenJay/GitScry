mod support;

use serde_json::Value;
use std::fs;
use support::{TestRepo, git, git_stdout};

fn commit(repo: &TestRepo, path: &str, text: &str) {
    fs::write(repo.dir.path().join(path), text).unwrap();
    git(repo.dir.path(), ["add", "--all"]);
    git(repo.dir.path(), ["commit", "-m", "touch"]);
}

fn commit_at(repo: &TestRepo, path: &str, text: &str, date: &str) -> String {
    let path = repo.dir.path().join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
    git(repo.dir.path(), ["add", "--all"]);
    let output = support::git_command(repo.dir.path())
        .args(["commit", "-m", "touch"])
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    repo.head()
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
fn retains_scoped_branch_touches_and_rename_lineage() {
    let repo = TestRepo::new();
    commit(&repo, "old", "one\ntwo\nthree\n");
    let from = repo.head();
    git(repo.dir.path(), ["checkout", "-b", "side"]);
    commit(&repo, "old", "one\ntwo\nthree\nbranch\n");
    fs::create_dir_all(repo.dir.path().join("src")).unwrap();
    git(repo.dir.path(), ["mv", "old", "src/new"]);
    record(&repo);
    let side_tip = repo.head();
    git(repo.dir.path(), ["checkout", "main"]);
    fs::create_dir_all(repo.dir.path().join("src")).unwrap();
    commit(&repo, "src/main-only", "main\n");
    let main_tip = repo.head();
    git(repo.dir.path(), ["merge", "--no-ff", "--no-commit", "side"]);
    fs::write(repo.dir.path().join("src/new"), "merge resolution\n").unwrap();
    fs::write(repo.dir.path().join("merge-only"), "merge\n").unwrap();
    record(&repo);
    repo.index();
    let json = report(&repo, &["hotspots", "--json"]);
    assert_eq!(count(&json, "src/new"), 3);
    assert_eq!(count(&json, "src/main-only"), 1);
    assert_eq!(json["files"].as_array().unwrap().len(), 2);
    let scoped = report(
        &repo,
        &[
            "hotspots",
            "--json",
            "--from-rev",
            &from,
            "--path-prefix",
            "src/",
        ],
    );
    assert_eq!(scoped["scope"]["from_rev"], from);
    assert_eq!(scoped["scope"]["path_prefix"], "src");
    assert_eq!(count(&scoped, "src/new"), 2);
    assert_eq!(count(&scoped, "src/main-only"), 1);
    assert_eq!(scoped["files"].as_array().unwrap().len(), 2);
    let invalid_range = repo.run(["hotspots", "--from-rev", &side_tip, "--to-rev", &main_tip]);
    assert_eq!(invalid_range.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&invalid_range.stderr)
            .contains("--from-rev must be an ancestor of --to-rev")
    );
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
        ["hotspots", "--since", "not-a-date"],
        ["hotspots", "--path-prefix", "../escape"],
    ] {
        assert_eq!(repo.run(args).status.code(), Some(2));
    }
    assert_eq!(
        repo.run(["hotspots", "--from-rev", "not-a-revision"])
            .status
            .code(),
        Some(2)
    );
    git(repo.dir.path(), ["checkout", "-b", "uncached"]);
    commit(&repo, "uncached", "revision not in published cache\n");
    let uncached = repo.head();
    let uncached_output = repo.run(["hotspots", "--to-rev", &uncached]);
    assert_eq!(uncached_output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&uncached_output.stderr)
            .contains("outside the published cache generation")
    );
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
    assert!(help.contains("--from-rev"));
    assert!(help.contains("--to-rev"));
    assert!(help.contains("--since"));
    assert!(help.contains("--until"));
    assert!(help.contains("--path-prefix"));
}

#[test]
fn merge_parent_lineage_uses_identity_not_only_path_presence() {
    for recreated in [false, true] {
        for reversed in [false, true] {
            let repo = TestRepo::new();
            commit(&repo, "old", "one\ntwo\nthree\n");
            git(repo.dir.path(), ["branch", "side"]);
            if !recreated {
                git(repo.dir.path(), ["mv", "old", "new"]);
                record(&repo);
            }
            let main = repo.head();
            git(repo.dir.path(), ["checkout", "side"]);
            if recreated {
                git(repo.dir.path(), ["rm", "old"]);
                record(&repo);
            }
            commit(&repo, "old", "one\ntwo\nthree\nbranch\n");
            let side = repo.head();
            let (first, second) = if reversed {
                (&side, &main)
            } else {
                (&main, &side)
            };
            // A real merge DAG with a deterministic resolution: keep the first parent's tree.
            let tree = git_stdout(repo.dir.path(), ["rev-parse", &format!("{first}^{{tree}}")]);
            let merged = git_stdout(
                repo.dir.path(),
                [
                    "commit-tree",
                    &tree,
                    "-p",
                    first,
                    "-p",
                    second,
                    "-m",
                    "merge",
                ],
            );
            git(repo.dir.path(), ["checkout", "main"]);
            git(repo.dir.path(), ["reset", "--hard", &merged]);
            repo.index();
            let json = report(&repo, &["hotspots", "--json"]);
            let path = if !recreated && !reversed {
                "new"
            } else {
                "old"
            };
            assert_eq!(
                count(&json, path),
                if recreated { 1 } else { 3 },
                "recreated={recreated}, reversed={reversed}"
            );
        }
    }
}

#[test]
fn scopes_hotspots_by_revisions_committer_time_and_target_directory() {
    let repo = TestRepo::new();
    let from = commit_at(&repo, "src/file.rs", "one\n", "2020-01-01T00:00:00Z");
    commit_at(&repo, "src/file.rs", "two\n", "2020-01-02T00:00:00Z");
    commit_at(
        &repo,
        "src-old/file.rs",
        "outside\n",
        "2020-01-02T12:00:00Z",
    );
    fs::write(repo.dir.path().join("src/zero.rs"), "no eligible touch\n").unwrap();
    let target = commit_at(&repo, "src/file.rs", "three\n", "2020-01-03T00:00:00Z");
    let cache_tip = commit_at(&repo, "src/new.rs", "new\n", "2020-01-04T00:00:00Z");
    repo.index();

    let json = report(
        &repo,
        &[
            "hotspots",
            "--json",
            "--from-rev",
            &from,
            "--to-rev",
            &target,
            "--since",
            "2020-01-01",
            "--until",
            "2020-01-02",
            "--path-prefix",
            "src/",
        ],
    );
    assert_eq!(json["scope"]["target_rev"], target);
    assert_eq!(json["scope"]["cache_tip"], cache_tip);
    assert_eq!(json["scope"]["from_rev"], from);
    assert_eq!(json["scope"]["to_rev"], target);
    assert_eq!(json["scope"]["since"], "2020-01-01");
    assert_eq!(json["scope"]["until"], "2020-01-02");
    assert_eq!(json["scope"]["path_prefix"], "src");
    assert_eq!(json["files"].as_array().unwrap().len(), 1, "{json}");
    assert_eq!(json["files"][0]["path"], "src/file.rs");
    assert_eq!(json["files"][0]["touching_commits"], 1, "{json}");
    assert_eq!(json["files"][0]["last_changed"], "2020-01-02T00:00:00Z");

    let text = repo.run([
        "hotspots",
        "--from-rev",
        &from,
        "--to-rev",
        &target,
        "--since",
        "2020-01-01",
        "--until",
        "2020-01-02",
        "--path-prefix",
        "src/",
    ]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.contains(&format!("after {from} through {target} inclusive")));
    assert!(text.contains(&format!("published cache tip {cache_tip}")));
    assert!(text.contains("committer time 2020-01-01 through 2020-01-02 inclusive"));
    assert!(text.contains("target directory prefix `src`"));
    assert!(text.contains("1  2020-01-02T00:00:00Z  src/file.rs"));
    assert!(!text.contains("src-old/file.rs"));
    assert!(!text.contains("src/new.rs"));
}

#[test]
fn directory_prefix_matching_is_case_sensitive() {
    let repo = TestRepo::new();
    commit_at(&repo, "SRC/file.rs", "one\n", "2020-01-01T00:00:00Z");
    repo.index();
    let json = report(&repo, &["hotspots", "--json", "--path-prefix", "src"]);
    assert!(json["files"].as_array().unwrap().is_empty(), "{json}");
}
