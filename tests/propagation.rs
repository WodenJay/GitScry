//! Propagation command behavior through the real CLI.
mod support;

use serde_json::Value;
use std::{fs, process::Output};
use support::{TestRepo, git, git_stdout};

fn commit(repo: &TestRepo, path: &str, text: &str, message: &str) -> String {
    let file_path = repo.dir.path().join(path);
    fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    fs::write(file_path, text).unwrap();
    git(repo.dir.path(), ["add", path]);
    git(repo.dir.path(), ["commit", "-m", message]);
    repo.head()
}

fn merge_no_ff(repo: &TestRepo, branch: &str) {
    git(repo.dir.path(), ["merge", "--no-ff", branch, "-m", "merge"]);
}

fn json(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// base - side (branched from base) with main carrying an unrelated commit,
/// then main merges side with --no-ff, and `unrelated` branches off base.
fn fixture() -> (TestRepo, String, String, String) {
    let repo = TestRepo::new();
    let base = commit(&repo, "a.txt", "base\n", "base");
    git(repo.dir.path(), ["checkout", "-b", "side"]);
    let side = commit(&repo, "b.txt", "side\n", "side change");
    git(repo.dir.path(), ["checkout", "main"]);
    commit(&repo, "c.txt", "main\n", "main change");
    merge_no_ff(&repo, "side");
    git(repo.dir.path(), ["checkout", "-b", "unrelated", &base]);
    commit(&repo, "d.txt", "unrelated\n", "unrelated change");
    git(repo.dir.path(), ["checkout", "main"]);
    repo.index();
    let head = repo.head();
    (repo, base, side, head)
}

#[test]
fn reports_contained_and_indeterminate_across_multiple_targets_in_requested_order() {
    let (repo, base, side, merged) = fixture();

    let value = json(repo.run([
        "propagation",
        &side,
        "--to",
        "main",
        "--to",
        "unrelated",
        "--to",
        &base,
        "--json",
    ]));

    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["source"], side);
    assert_eq!(value["coverage_complete"], true);
    assert_eq!(value["warnings"].as_array().unwrap().len(), 0);

    let targets = value["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 3);
    assert_eq!(targets[0]["target_ref"], "main");
    assert_eq!(targets[1]["target_ref"], "unrelated");
    assert_eq!(targets[2]["target_ref"], base.clone());

    assert_eq!(targets[0]["status"], "contained");
    assert_eq!(targets[0]["target_oid"], merged);
    assert_eq!(targets[0]["source_oid"], side);
    assert_eq!(targets[0]["contained_by"], serde_json::json!([side]));
    assert_eq!(targets[0]["reason"], Value::Null);

    // Not an ancestor, and patch equivalence has not been checked yet.
    assert_eq!(targets[1]["status"], "indeterminate");
    assert_eq!(targets[1]["contained_by"], serde_json::json!([]));
    let reason = targets[1]["reason"].as_str().unwrap();
    assert!(reason.contains("patch equivalence"), "{reason}");

    // source=side is not an ancestor of target=base (wrong direction).
    assert_eq!(targets[2]["status"], "indeterminate");
    assert_eq!(targets[2]["contained_by"], serde_json::json!([]));

    let main_oid = git_stdout(repo.dir.path(), ["rev-parse", "main"]);
    assert_eq!(targets[0]["target_oid"], main_oid);
    assert_eq!(
        targets[1]["target_oid"],
        git_stdout(repo.dir.path(), ["rev-parse", "unrelated"])
    );
    assert_eq!(targets[2]["target_oid"], base);
}

#[test]
fn human_output_names_status_and_reason_without_claiming_not_found() {
    let (repo, _, side, _) = fixture();
    let output = repo.run(["propagation", &side, "--to", "main", "--to", "unrelated"]);
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("contained"), "{text}");
    assert!(text.contains("indeterminate"), "{text}");
    assert!(text.contains("patch equivalence"), "{text}");
    assert!(!text.contains("not_found"), "{text}");
}

#[test]
fn containment_through_merged_non_first_parent_ancestry() {
    // The side commit reached main only through the merge commit's second parent.
    let (repo, _, side, _) = fixture();
    let value = json(repo.run(["propagation", &side, "--to", "main", "--json"]));
    assert_eq!(value["targets"][0]["status"], "contained");
}

#[test]
fn source_merge_commit_is_contained_by_target() {
    let (repo, _, _, merged) = fixture();
    // merged is the merge commit itself; its OID is directly reachable from main.
    let value = json(repo.run(["propagation", &merged, "--to", "main", "--json"]));
    let target = &value["targets"][0];
    assert_eq!(target["status"], "contained");
    assert_eq!(target["contained_by"], serde_json::json!([merged]));
}

#[test]
fn source_merge_commit_unreachable_from_independent_branch_is_indeterminate() {
    let (repo, _, _, merged) = fixture();
    let value = json(repo.run(["propagation", &merged, "--to", "unrelated", "--json"]));
    let target = &value["targets"][0];
    assert_eq!(target["status"], "indeterminate");
    let reason = target["reason"].as_str().unwrap();
    assert!(reason.contains("patch equivalence"), "{reason}");
}

#[test]
fn requires_at_least_one_target_and_fails_invalid_revisions_as_input_errors() {
    let (repo, _, side, _) = fixture();
    repo.index();

    let missing_target = repo.run(["propagation", &side]);
    assert_eq!(missing_target.status.code(), Some(2));
    assert!(missing_target.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&missing_target.stderr);
    assert!(stderr.contains("--to"), "{stderr}");

    let invalid_source = repo.run(["propagation", "nosuchrev", "--to", "main"]);
    assert_eq!(invalid_source.status.code(), Some(2));
    assert!(invalid_source.stdout.is_empty());

    let invalid_target = repo.run(["propagation", &side, "--to", "nosuchref"]);
    assert_eq!(invalid_target.status.code(), Some(2));
    assert!(invalid_target.stdout.is_empty());
}

#[test]
fn rejects_missing_published_cache_without_initializing_it() {
    let (repo, _, side, _) = fixture();
    fs::remove_dir_all(repo.cache_dir()).unwrap();

    let output = repo.run(["propagation", &side, "--to", "main"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("run `gitscry index` first"));
    assert!(!repo.cache_dir().exists());
}

#[test]
fn refreshes_explicit_targets_while_head_stays_unchanged() {
    let (repo, base, side, _) = fixture();
    // Index only from HEAD (main), then check an uncached explicit target branch.
    repo.index();
    let head_before = repo.head();

    let value = json(repo.run(["propagation", &side, "--to", "unrelated", "--json"]));
    assert_eq!(value["targets"][0]["status"], "indeterminate");
    assert_eq!(
        value["targets"][0]["target_oid"],
        git_stdout(repo.dir.path(), ["rev-parse", "unrelated"])
    );

    // HEAD never moved and the unrelated branch worktree still points at main.
    assert_eq!(repo.head(), head_before);
    assert_eq!(
        git_stdout(repo.dir.path(), ["symbolic-ref", "--short", "HEAD"]),
        "main"
    );

    // The query made the explicit target's history queryable without touching HEAD.
    let base_value = json(repo.run(["propagation", &base, "--to", "unrelated", "--json"]));
    assert_eq!(base_value["targets"][0]["status"], "contained");
}

#[test]
fn duplicate_targets_share_one_refresh_and_keep_each_result() {
    let (repo, _, side, _) = fixture();
    let value = json(repo.run([
        "propagation",
        &side,
        "--to",
        "main",
        "--to",
        "main",
        "--json",
    ]));
    let targets = value["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 2);
    assert!(targets.iter().all(|t| t["status"] == "contained"));
}

#[test]
fn deep_history_branch_resolves_containment_after_refresh() {
    // A target branched from the source after indexing: the query must refresh
    // the explicit target and then report containment without moving HEAD.
    let (repo, _, side, _) = fixture();
    let head_before = repo.head();

    git(repo.dir.path(), ["checkout", "-b", "distant", &side]);
    let distant_head = commit(&repo, "e.txt", "distant\n", "distant change");
    git(repo.dir.path(), ["checkout", "main"]);

    let value = json(repo.run(["propagation", &side, "--to", "distant", "--json"]));
    let target = &value["targets"][0];
    assert_eq!(target["target_ref"], "distant");
    assert_eq!(target["target_oid"], distant_head);
    assert_eq!(target["status"], "contained");
    assert_eq!(repo.head(), head_before);
}
