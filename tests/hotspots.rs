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
fn ranks_repeated_touches_after_refreshing_current_head() {
    let repo = TestRepo::new();
    commit(&repo, "often", "one\n");
    commit(&repo, "once", &"large rewrite\n".repeat(100));
    commit(&repo, "often", "two\n");
    repo.index();
    let cache_tip = repo.head();
    commit(&repo, "unindexed", "ignored\n");
    let target = repo.head();
    assert_ne!(cache_tip, target);
    fs::remove_file(repo.dir.path().join("often")).unwrap();
    let json = report(&repo, &["hotspots", "--json"]);
    assert_eq!(json["scope"]["target_rev"], target);
    assert_eq!(json["scope"]["cache_tip"], target);
    assert_eq!(json["scope"]["coverage_complete"], true);
    assert!(
        !json["coverage"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning
                .as_str()
                .unwrap()
                .contains("incomplete history coverage"))
    );
    assert_eq!(json["files"].as_array().unwrap().len(), 3);
    assert_eq!(json["files"][0]["path"], "often");
    assert_eq!(json["files"][0]["touching_commits"], 2);
    assert!(
        json["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["path"] == "unindexed")
    );
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
    assert_eq!(json["files"][0]["additions"], 2);
    assert_eq!(json["files"][0]["deletions"], 1);
    assert_eq!(json["files"][0]["churn_complete"], true);
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
fn reports_exact_textual_additions_and_deletions_for_created_and_modified_files() {
    let repo = TestRepo::new();
    commit(&repo, "often", "one\ntwo\nthree\n");
    commit(&repo, "often", "one\nfour\n");
    commit(&repo, "created", "first\nsecond\n");
    repo.index();

    let json = report(&repo, &["hotspots", "--json"]);
    let files = json["files"].as_array().unwrap();
    let often = files.iter().find(|file| file["path"] == "often").unwrap();
    assert_eq!(often["additions"], 4);
    assert_eq!(often["deletions"], 2);
    assert_eq!(often["churn_complete"], true);
    let created = files.iter().find(|file| file["path"] == "created").unwrap();
    assert_eq!(created["additions"], 2);
    assert_eq!(created["deletions"], 0);
    assert_eq!(created["churn_complete"], true);

    let text = String::from_utf8(repo.run(["hotspots"]).stdout).unwrap();
    assert!(text.contains("Touches  +lines  -lines"));
    assert!(text.contains("Textual churn"));
    assert!(text.contains("4  2  "));
    assert!(text.contains("2  0  "));
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

    let files = json["files"].as_array().unwrap();
    let renamed = files.iter().find(|file| file["path"] == "new").unwrap();
    assert_eq!(renamed["additions"], 4);
    assert_eq!(renamed["deletions"], 0);
    assert_eq!(renamed["churn_complete"], true);
    let copy = files.iter().find(|file| file["path"] == "copy").unwrap();
    assert_eq!(copy["additions"], 4);
    assert_eq!(copy["deletions"], 0);
    let recreated = files.iter().find(|file| file["path"] == "reused").unwrap();
    assert_eq!(recreated["additions"], 1);
    assert_eq!(recreated["deletions"], 0);
    assert_eq!(recreated["churn_complete"], true);
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
    let new = json["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == "src/new")
        .unwrap();
    assert_eq!(new["additions"], 4);
    assert_eq!(new["deletions"], 0);
    assert_eq!(new["churn_complete"], true);
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
    let scoped_new = scoped["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == "src/new")
        .unwrap();
    assert_eq!(scoped_new["additions"], 1);
    assert_eq!(scoped_new["deletions"], 0);
    assert_eq!(scoped_new["churn_complete"], true);
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
fn marks_gitlink_churn_unavailable_when_path_becomes_a_file() {
    let repo = TestRepo::new();
    commit(&repo, "seed", "base\n");

    let first_link = format!("160000,{},module", repo.head());
    git(
        repo.dir.path(),
        ["update-index", "--add", "--cacheinfo", first_link.as_str()],
    );
    git(repo.dir.path(), ["commit", "-m", "add gitlink"]);

    let second_link = format!("160000,{},module", repo.head());
    git(
        repo.dir.path(),
        ["update-index", "--cacheinfo", second_link.as_str()],
    );
    git(repo.dir.path(), ["commit", "-m", "update gitlink"]);

    git(repo.dir.path(), ["rm", "--cached", "module"]);
    fs::write(repo.dir.path().join("module"), "").unwrap();
    git(repo.dir.path(), ["add", "module"]);
    git(
        repo.dir.path(),
        ["commit", "-m", "replace gitlink with empty file"],
    );
    repo.index();

    let json = report(&repo, &["hotspots", "--json"]);
    let module = json["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == "module")
        .unwrap();
    assert_eq!(module["touching_commits"], 3);

    assert_eq!(module["additions"], Value::Null);
    assert_eq!(module["deletions"], Value::Null);
    assert_eq!(module["churn_complete"], false);
}
#[test]
fn reports_empty_file_and_rename_as_complete_zero_churn() {
    let repo = TestRepo::new();
    commit(&repo, "empty", "");

    git(repo.dir.path(), ["mv", "empty", "renamed-empty"]);
    record(&repo);
    repo.index();

    let json = report(&repo, &["hotspots", "--json"]);
    let empty = &json["files"][0];
    assert_eq!(empty["path"], "renamed-empty");
    assert_eq!(empty["touching_commits"], 2);
    assert_eq!(empty["additions"], 0);
    assert_eq!(empty["deletions"], 0);
    assert_eq!(empty["churn_complete"], true);

    let text = String::from_utf8(repo.run(["hotspots"]).stdout).unwrap();
    let row = text
        .lines()
        .find(|line| line.ends_with(" renamed-empty"))
        .unwrap();
    assert!(row.contains("2  0  0  "));
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
    assert_eq!(files[0]["additions"], Value::Null);
    assert_eq!(files[0]["deletions"], Value::Null);
    assert_eq!(files[0]["churn_complete"], false);

    let text = String::from_utf8(repo.run(["hotspots"]).stdout).unwrap();
    let binary = text.lines().find(|line| line.ends_with(" binary")).unwrap();
    assert!(binary.contains("—  —"));
    assert_eq!(files[2]["additions"], 1);
    assert_eq!(files[2]["deletions"], 0);
    assert_eq!(files[2]["churn_complete"], true);
}
#[test]
fn marks_partial_textual_churn_when_binary_history_is_unavailable() {
    let repo = TestRepo::new();
    commit(&repo, "mixed", "first\nsecond\n");
    commit(&repo, "mixed", "a\0b");
    repo.index();

    let json = report(&repo, &["hotspots", "--json"]);
    let mixed = json["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == "mixed")
        .unwrap();
    assert_eq!(mixed["touching_commits"], 2);
    assert_eq!(mixed["additions"], 2);
    assert_eq!(mixed["deletions"], 0);
    assert_eq!(mixed["churn_complete"], false);

    let text = String::from_utf8(repo.run(["hotspots"]).stdout).unwrap();
    let row = text.lines().find(|line| line.ends_with(" mixed")).unwrap();
    assert!(row.contains("2*  0*"));
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
    let after_uncached = report(
        &repo,
        &["hotspots", "--to-rev", &uncached, "--json", "--limit", "30"],
    );
    assert_eq!(after_uncached["scope"]["target_rev"], uncached);
    assert_eq!(after_uncached["scope"]["cache_tip"], uncached);
    assert_eq!(after_uncached["scope"]["coverage_complete"], true);
    assert_eq!(count(&after_uncached, "uncached"), 1);
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
    assert!(help.contains("churn_complete"));
    assert!(help.contains("unavailable counts with"));
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
    assert_eq!(json["files"][0]["additions"], 1);
    assert_eq!(json["files"][0]["deletions"], 1);
    assert_eq!(json["files"][0]["churn_complete"], true);

    let target_scope = report(
        &repo,
        &[
            "hotspots",
            "--json",
            "--to-rev",
            &target,
            "--path-prefix",
            "src/",
        ],
    );
    assert_eq!(target_scope["scope"]["target_rev"], target);
    assert_eq!(target_scope["scope"]["cache_tip"], cache_tip);
    assert_eq!(count(&target_scope, "src/file.rs"), 3);
    assert_eq!(count(&target_scope, "src/zero.rs"), 1);
    assert_eq!(target_scope["files"].as_array().unwrap().len(), 2);

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
    assert!(text.contains("1  1  1  2020-01-02T00:00:00Z  src/file.rs"));
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

#[test]
fn includes_cached_histories_from_both_parents_of_uncached_head_merge() {
    let repo = TestRepo::new();
    commit_at(&repo, "root.rs", "root\n", "2020-01-01T00:00:00Z");
    let base = repo.head();
    git(
        repo.dir.path(),
        ["checkout", "-b", "feature", base.as_str()],
    );
    commit_at(
        &repo,
        "feature-only.rs",
        "feature\n",
        "2020-01-02T00:00:00Z",
    );
    let feature_tip = repo.head();
    git(repo.dir.path(), ["checkout", "main"]);
    commit_at(&repo, "main-only.rs", "main\n", "2020-01-03T00:00:00Z");
    let main_tip = repo.head();
    git(
        repo.dir.path(),
        ["merge", "--no-ff", "feature", "-m", "cached merge"],
    );
    repo.index();
    let cache_tip = repo.head();

    git(repo.dir.path(), ["checkout", "--detach", main_tip.as_str()]);
    git(
        repo.dir.path(),
        ["merge", "--no-ff", "feature", "-m", "uncached merge"],
    );
    let head = repo.head();
    assert_ne!(head, cache_tip);

    let json = report(&repo, &["hotspots", "--json"]);
    assert_eq!(json["scope"]["target_rev"], head);
    assert_eq!(json["scope"]["cache_tip"], head);
    assert_eq!(json["scope"]["coverage_complete"], true);
    assert_eq!(count(&json, "feature-only.rs"), 1);
    assert_eq!(count(&json, "main-only.rs"), 1);
    assert_eq!(count(&json, "root.rs"), 1);
    assert_ne!(feature_tip, main_tip);
}
