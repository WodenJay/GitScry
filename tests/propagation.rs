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
fn reports_contained_and_not_found_across_multiple_targets_in_requested_order() {
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

    assert_eq!(value["schema_version"], 2);
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

    // Not an ancestor, and no whole-commit patch equivalent exists in a fully
    // available history: a completed search reports not_found without a reason.
    assert_eq!(targets[1]["status"], "not_found");
    assert_eq!(targets[1]["contained_by"], serde_json::json!([]));
    assert_eq!(targets[1]["equivalents"], serde_json::json!([]));
    assert_eq!(targets[1]["reason"], Value::Null);

    // source=side is not an ancestor of target=base (wrong direction) and the
    // whole-commit patches differ (b.txt vs base's a.txt); the search completes.
    assert_eq!(targets[2]["status"], "not_found");
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
fn human_output_names_status_without_hedging_a_completed_search() {
    let (repo, _, side, _) = fixture();
    let output = repo.run(["propagation", &side, "--to", "main", "--to", "unrelated"]);
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("contained"), "{text}");
    // A completed search over fully available history reports not_found, not a
    // vague indeterminate hedge.
    assert!(text.contains("not_found"), "{text}");
    assert!(!text.contains("indeterminate"), "{text}");
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
    assert!(!reason.is_empty(), "reason must explain the gap: {reason}");
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
    let (repo, base, _side, _) = fixture();
    // Index only from HEAD (main), then check an uncached explicit target branch.
    repo.index();
    let head_before = repo.head();

    let value = json(repo.run(["propagation", &_side, "--to", "unrelated", "--json"]));
    // The unrelated branch's history is fully available; its whole-commit patches
    // differ from side's, so the completed search reports not_found.
    assert_eq!(value["targets"][0]["status"], "not_found");
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

#[test]
fn cherry_picked_commit_with_a_new_message_is_an_equivalent() {
    let repo = TestRepo::new();
    let base = commit(&repo, "a.txt", "base\n", "base");
    git(repo.dir.path(), ["checkout", "-b", "source"]);
    let source = commit(&repo, "b.txt", "feature\n", "add feature");

    // The target replays the same whole-commit patch onto the same base, so the
    // patch identifier matches even though the commit SHA and message differ.
    git(repo.dir.path(), ["checkout", "-b", "target", &base]);
    git(repo.dir.path(), ["cherry-pick", "-x", &source]);
    let replayed = repo.head();
    assert_ne!(
        replayed, source,
        "the cherry-pick must be a distinct commit"
    );
    git(repo.dir.path(), ["checkout", "main"]);
    repo.index();

    let value = json(repo.run(["propagation", &source, "--to", "target", "--json"]));
    let target = &value["targets"][0];
    assert_eq!(target["status"], "equivalent");
    assert_eq!(target["equivalents"], serde_json::json!([replayed]));
    assert_eq!(target["contained_by"], serde_json::json!([]));
    assert_eq!(target["matching_method"], "patch-id --verbatim");
    assert!(
        target["patch_identifier"]
            .as_str()
            .is_some_and(|id| id.len() == 40)
    );
    assert_eq!(value["coverage_complete"], true);
}

#[test]
fn rebased_commit_is_an_equivalent() {
    let repo = TestRepo::new();
    let base = commit(&repo, "a.txt", "base\n", "base");
    git(repo.dir.path(), ["checkout", "-b", "feature"]);
    let source = commit(&repo, "s.txt", "feature\n", "add feature");

    git(repo.dir.path(), ["checkout", "-b", "target", &base]);
    commit(&repo, "t.txt", "target\n", "target change");
    // Rebase the feature commit onto the target: the patch content is unchanged.
    git(repo.dir.path(), ["checkout", "feature"]);
    git(repo.dir.path(), ["rebase", "target"]);
    let replayed = repo.head();
    assert_ne!(replayed, source, "the rebase must rewrite the commit");
    git(repo.dir.path(), ["checkout", "main"]);
    repo.index();

    let value = json(repo.run(["propagation", &source, "--to", "feature", "--json"]));
    let target = &value["targets"][0];
    assert_eq!(target["status"], "equivalent");
    assert_eq!(target["equivalents"], serde_json::json!([replayed]));
}

#[test]
fn historically_applied_patches_are_reported_in_history_order() {
    let repo = TestRepo::new();
    let base = commit(&repo, "a.txt", "base\n", "base");
    // A distinct source commit carrying the same patch.
    git(repo.dir.path(), ["checkout", "-b", "source"]);
    let source = commit(&repo, "b.txt", "feature\n", "add feature");

    // Apply, revert, then re-apply the patch on the target.
    git(repo.dir.path(), ["checkout", "-b", "target", &base]);
    let first = commit(&repo, "b.txt", "feature\n", "apply");
    git(repo.dir.path(), ["rm", "b.txt"]);
    git(repo.dir.path(), ["commit", "-m", "revert"]);
    let last = commit(&repo, "b.txt", "feature\n", "reapply");
    git(repo.dir.path(), ["checkout", "main"]);
    repo.index();

    let value = json(repo.run(["propagation", &source, "--to", "target", "--json"]));
    let target = &value["targets"][0];
    assert_eq!(target["status"], "equivalent");
    // Both historical applications are reported, in deterministic target-history order.
    assert_eq!(target["equivalents"], serde_json::json!([first, last]));
}

#[test]
fn equivalent_reachable_only_through_a_merge_is_found_once() {
    let repo = TestRepo::new();
    let base = commit(&repo, "a.txt", "base\n", "base");
    git(repo.dir.path(), ["checkout", "-b", "dup", &base]);
    let duplicate = commit(&repo, "b.txt", "feature\n", "duplicate feature");
    git(repo.dir.path(), ["checkout", "-b", "source", &base]);
    let source = commit(&repo, "b.txt", "feature\n", "add feature");

    git(repo.dir.path(), ["checkout", "-b", "target", &base]);
    commit(&repo, "c.txt", "target\n", "target change");
    merge_no_ff(&repo, "dup");
    git(repo.dir.path(), ["checkout", "main"]);
    repo.index();

    let value = json(repo.run(["propagation", &source, "--to", "target", "--json"]));
    let target = &value["targets"][0];
    assert_eq!(target["status"], "equivalent");
    // The commit is reachable only through the merge's second parent, and appears once.
    assert_eq!(target["equivalents"], serde_json::json!([duplicate]));
}

#[test]
fn whitespace_difference_is_not_equivalent() {
    let repo = TestRepo::new();
    let base = commit(&repo, "a.txt", "base\n", "base");
    git(repo.dir.path(), ["checkout", "-b", "source"]);
    let source = commit(&repo, "w.txt", "feature\n", "add feature");

    git(repo.dir.path(), ["checkout", "-b", "target", &base]);
    commit(
        &repo,
        "w.txt",
        "feature \n",
        "add feature with trailing space",
    );
    git(repo.dir.path(), ["checkout", "main"]);
    repo.index();

    let value = json(repo.run(["propagation", &source, "--to", "target", "--json"]));
    let target = &value["targets"][0];
    assert_eq!(target["status"], "not_found");
    assert_eq!(target["equivalents"], serde_json::json!([]));
    assert_eq!(value["coverage_complete"], true);
}

#[test]
fn partial_backport_is_not_equivalent() {
    let repo = TestRepo::new();
    let base = commit(&repo, "a.txt", "base\n", "base");
    git(repo.dir.path(), ["checkout", "-b", "source"]);
    fs::write(repo.dir.path().join("x.txt"), "x\n").unwrap();
    fs::write(repo.dir.path().join("y.txt"), "y\n").unwrap();
    git(repo.dir.path(), ["add", "x.txt", "y.txt"]);
    git(repo.dir.path(), ["commit", "-m", "add two files"]);
    let source = repo.head();

    // The target applies only one of the two files: the whole-commit patch differs.
    git(repo.dir.path(), ["checkout", "-b", "target", &base]);
    commit(&repo, "x.txt", "x\n", "add one file");
    git(repo.dir.path(), ["checkout", "main"]);
    repo.index();

    let value = json(repo.run(["propagation", &source, "--to", "target", "--json"]));
    assert_eq!(value["targets"][0]["status"], "not_found");
}

#[test]
fn squashed_commits_are_not_equivalents() {
    let repo = TestRepo::new();
    let base = commit(&repo, "a.txt", "base\n", "base");
    git(repo.dir.path(), ["checkout", "-b", "source"]);
    let first = commit(&repo, "x.txt", "x\n", "add x");
    let second = commit(&repo, "y.txt", "y\n", "add y");

    // The target squashes both source commits into a single commit.
    git(repo.dir.path(), ["checkout", "-b", "target", &base]);
    fs::write(repo.dir.path().join("x.txt"), "x\n").unwrap();
    fs::write(repo.dir.path().join("y.txt"), "y\n").unwrap();
    git(repo.dir.path(), ["add", "x.txt", "y.txt"]);
    git(repo.dir.path(), ["commit", "-m", "add x and y"]);
    git(repo.dir.path(), ["checkout", "main"]);
    repo.index();

    for source in [&first, &second] {
        let value = json(repo.run(["propagation", source, "--to", "target", "--json"]));
        assert_eq!(
            value["targets"][0]["status"], "not_found",
            "a squashed commit must not match either source commit"
        );
    }
}

#[test]
fn equivalent_is_reported_even_when_other_history_is_incomplete() {
    let repo = TestRepo::new();
    let base = commit(&repo, "a.txt", "base\n", "base");
    git(repo.dir.path(), ["checkout", "-b", "source"]);
    let source = commit(&repo, "b.txt", "feature\n", "add feature");
    git(repo.dir.path(), ["checkout", "-b", "target", &base]);
    let duplicate = commit(&repo, "b.txt", "feature\n", "duplicate feature");
    git(repo.dir.path(), ["checkout", "main"]);
    repo.index();

    // Mark the root as a shallow boundary so the target history is incomplete.
    fs::write(repo.common_dir().join("shallow"), format!("{base}\n")).unwrap();

    let value = json(repo.run(["propagation", &source, "--to", "target", "--json"]));
    let target = &value["targets"][0];
    assert_eq!(target["status"], "equivalent");
    assert_eq!(target["equivalents"], serde_json::json!([duplicate]));
    assert!(
        target["reason"]
            .as_str()
            .is_some_and(|reason| !reason.is_empty()),
        "incompleteness is reported alongside the positive match"
    );
    assert_eq!(value["coverage_complete"], false);
}

#[test]
fn unreadable_candidate_patch_reports_indeterminate_not_not_found() {
    let repo = TestRepo::new();
    let base = commit(&repo, "a.txt", "base\n", "base");
    git(repo.dir.path(), ["checkout", "-b", "source"]);
    let source = commit(&repo, "s.txt", "source\n", "add source");
    git(repo.dir.path(), ["checkout", "-b", "target", &base]);
    let target = commit(&repo, "t.txt", "target\n", "add target");
    git(repo.dir.path(), ["checkout", "main"]);
    repo.index();

    // Remove a blob the target candidate needs so its patch cannot be read.
    let blob = git_stdout(repo.dir.path(), ["rev-parse", &format!("{target}:t.txt")]);
    let object = repo
        .dir
        .path()
        .join(".git/objects")
        .join(&blob[..2])
        .join(&blob[2..]);
    fs::remove_file(object).expect("remove blob object");

    let value = json(repo.run(["propagation", &source, "--to", "target", "--json"]));
    let result = &value["targets"][0];
    assert_eq!(result["status"], "indeterminate");
    assert!(
        result["reason"]
            .as_str()
            .is_some_and(|reason| !reason.is_empty()),
        "an unreadable candidate is an inspection gap, not a negative"
    );
    assert_eq!(value["coverage_complete"], false);
}

#[test]
fn source_commit_without_a_patch_is_indeterminate() {
    let repo = TestRepo::new();
    let base = commit(&repo, "a.txt", "base\n", "base");
    git(repo.dir.path(), ["checkout", "-b", "source"]);
    git(repo.dir.path(), ["commit", "--allow-empty", "-m", "empty"]);
    let source = repo.head();
    git(repo.dir.path(), ["checkout", "-b", "target", &base]);
    commit(&repo, "c.txt", "target\n", "target change");
    git(repo.dir.path(), ["checkout", "main"]);
    repo.index();

    let value = json(repo.run(["propagation", &source, "--to", "target", "--json"]));
    let target = &value["targets"][0];
    assert_eq!(target["status"], "indeterminate");
    assert!(
        target["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("no whole-commit patch"))
    );
}

#[test]
fn ambient_diff_configuration_does_not_change_equivalence() {
    let repo = TestRepo::new();
    let base = commit(&repo, "a.txt", "base\n", "base");
    git(repo.dir.path(), ["checkout", "-b", "source"]);
    let source = commit(&repo, "b.txt", "feature\n", "add feature");
    git(repo.dir.path(), ["checkout", "-b", "target", &base]);
    git(repo.dir.path(), ["cherry-pick", "-x", &source]);
    let replayed = repo.head();
    assert_ne!(
        replayed, source,
        "the cherry-pick must be a distinct commit"
    );
    git(repo.dir.path(), ["checkout", "main"]);
    repo.index();

    let clean = json(repo.run(["propagation", &source, "--to", "target", "--json"]));
    assert_eq!(clean["targets"][0]["status"], "equivalent");

    // Hostile ambient diff configuration must not change the identifier or result.
    for (key, value) in [
        ("diff.context", "25"),
        ("diff.renames", "copies"),
        ("diff.noprefix", "true"),
        ("diff.mnemonicPrefix", "true"),
        ("diff.algorithm", "histogram"),
        ("diff.indentHeuristic", "true"),
    ] {
        git(repo.dir.path(), ["config", key, value]);
    }

    let polluted = json(repo.run(["propagation", &source, "--to", "target", "--json"]));
    assert_eq!(polluted["targets"][0]["status"], "equivalent");
    assert_eq!(
        polluted["targets"][0]["equivalents"],
        serde_json::json!([replayed])
    );
    assert_eq!(
        polluted["targets"][0]["patch_identifier"],
        clean["targets"][0]["patch_identifier"]
    );
}
