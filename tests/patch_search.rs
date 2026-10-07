mod support;

use serde_json::Value;
use std::fs;
use support::{TestRepo, git};

fn commit(repo: &TestRepo, path: &str, content: &[u8], message: &str) -> String {
    fs::write(repo.dir.path().join(path), content).unwrap();
    git(repo.dir.path(), ["add", path]);
    git(repo.dir.path(), ["commit", "-m", message]);
    repo.head()
}

fn search(repo: &TestRepo, revision: &str, extra: &[&str]) -> Value {
    let mut args = vec![
        "search",
        "--patch-of",
        revision,
        "--relation",
        "equivalent",
        "--json",
    ];
    args.extend_from_slice(extra);
    let output = repo.run(args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn discovers_complete_equivalents_on_other_branches_without_self_matches() {
    let repo = TestRepo::new();
    let base = commit(&repo, "file", b"old\n", "Base");
    let query = commit(&repo, "file", b"new\n", "Query patch");
    repo.index();
    git(repo.dir.path(), ["checkout", "-b", "release", &base]);
    let equivalent = commit(&repo, "file", b"new\n", "Unrelated message");
    git(repo.dir.path(), ["checkout", "main"]);
    let value = search(&repo, &query, &[]);
    assert_eq!(value["kind"], "patch-search");
    assert_eq!(value["matched_count"], 1);
    assert_eq!(value["matches"][0]["commit_id"], equivalent);
    assert_eq!(value["matches"][0]["relation"], "equivalent");
    assert_eq!(value["matches"][0]["comparison_basis"], "first_parent");
    assert_eq!(value["scope"]["coverage_complete"], true);
    assert_eq!(value["query"]["commit_id"], query);
    assert_eq!(value["query"]["integrity"], "complete");
    assert_eq!(search(&repo, &query, &[]), value);
    assert_eq!(
        search(&repo, &query, &["--to-rev", "main"])["matched_count"],
        0
    );
}

#[test]
fn preserves_complete_content_whitespace_paths_newlines_and_change_structure() {
    let repo = TestRepo::new();
    let base = commit(&repo, "file", b"prefix\nold\nend\n", "Base");
    let query = commit(&repo, "file", b"prefix\nnew\nend\n", "Query");
    repo.index();
    git(repo.dir.path(), ["checkout", "-b", "shifted", &base]);
    commit(
        &repo,
        "file",
        b"extra context\nprefix\nold\nend\n",
        "Shift location",
    );
    let shifted = commit(
        &repo,
        "file",
        b"extra context\nprefix\nnew\nend\n",
        "Equivalent at other line",
    );
    git(repo.dir.path(), ["checkout", "-b", "whitespace", &base]);
    commit(&repo, "file", b"prefix\nnew \nend\n", "Whitespace differs");
    git(repo.dir.path(), ["checkout", "-b", "extra", &base]);
    fs::write(repo.dir.path().join("other"), b"extra\n").unwrap();
    git(repo.dir.path(), ["add", "other"]);
    commit(&repo, "file", b"prefix\nnew\nend\n", "Extra file");
    git(repo.dir.path(), ["checkout", "main"]);
    let value = search(&repo, &query, &[]);
    assert_eq!(value["matched_count"], 1);
    assert_eq!(value["matches"][0]["commit_id"], shifted);
    let no_newline_base = commit(&repo, "tail", b"old", "Tail base");
    let tail = commit(&repo, "tail", b"new", "No newline query");
    git(
        repo.dir.path(),
        ["checkout", "-b", "newline", &no_newline_base],
    );
    commit(&repo, "tail", b"new\n", "Newline differs");
    assert_eq!(search(&repo, &tail, &[])["matched_count"], 0);
}

#[test]
fn indeterminate_query_and_candidates_never_certify_readable_fragments() {
    let repo = TestRepo::new();
    let base = commit(&repo, "file", b"old\n", "Base");
    let query = commit(&repo, "file", b"new\n", "Query");
    repo.index();
    git(repo.dir.path(), ["checkout", "-b", "binary", &base]);
    fs::write(repo.dir.path().join("binary"), b"\0unreadable").unwrap();
    git(repo.dir.path(), ["add", "binary"]);
    let binary = commit(&repo, "file", b"new\n", "Text plus binary");
    let value = search(&repo, &query, &[]);
    assert_eq!(value["matched_count"], 0);
    assert_eq!(value["scope"]["coverage_complete"], false);
    assert_eq!(value["scope"]["indeterminate"][0]["commit_id"], binary);
    let value = search(&repo, &binary, &[]);
    assert_eq!(value["query"]["integrity"], "indeterminate");
    assert_eq!(value["matched_count"], 0);
    assert_eq!(value["scope"]["coverage_complete"], false);
}

#[test]
fn result_limits_only_bound_presentation_and_cutoffs_disclose_gaps() {
    let repo = TestRepo::new();
    let base = commit(&repo, "file", b"old\n", "Base");
    let query = commit(&repo, "file", b"new\n", "Query");
    repo.index();
    for branch in ["release-a", "release-b", "release-c"] {
        git(repo.dir.path(), ["checkout", "-b", branch, &base]);
        commit(&repo, "file", b"new\n", branch);
    }
    let value = search(&repo, &query, &["--limit", "1"]);
    assert_eq!(value["matched_count"], 3);
    assert_eq!(value["returned_count"], 1);
    assert_eq!(value["scope"]["unexamined_count"], 0);
    assert_eq!(value["scope"]["coverage_complete"], true);
    let value = search(&repo, &query, &["--max-patch-checks", "1"]);
    assert_eq!(value["scope"]["checked_count"], 1);
    assert!(value["scope"]["unexamined_count"].as_u64().unwrap() > 0);
    assert_eq!(value["scope"]["coverage_complete"], false);
}

#[test]
fn patch_input_rejects_partial_paths_other_modes_and_future_relations() {
    let repo = TestRepo::new();
    for extra in [
        vec!["words"],
        vec!["--code", "text"],
        vec!["--code-regex", "text"],
        vec!["--code-file", "file"],
        vec!["--path", "file"],
        vec!["--hybrid"],
        vec!["--github-links"],
        vec!["--relation", "inverse"],
    ] {
        let mut args = vec!["search", "--patch-of", "HEAD"];
        args.extend(extra);
        assert!(!repo.run(args).status.success());
    }
    assert!(
        !repo
            .run(["search", "word", "--relation", "equivalent"])
            .status
            .success()
    );
}

#[test]
fn roots_empty_patches_merges_and_fetched_remote_tips_have_explicit_bases() {
    let repo = TestRepo::new();
    let root = commit(&repo, "file", b"old\n", "Root");
    repo.index();
    git(repo.dir.path(), ["checkout", "--orphan", "other-root"]);
    git(repo.dir.path(), ["rm", "-rf", "."]);
    let other_root = commit(&repo, "file", b"old\n", "Independent root");
    let value = search(&repo, &root, &[]);
    assert_eq!(value["matched_count"], 1);
    assert_eq!(value["matches"][0]["commit_id"], other_root);
    assert_eq!(value["matches"][0]["comparison_basis"], "root_introduction");
    git(repo.dir.path(), ["checkout", "main"]);
    git(repo.dir.path(), ["checkout", "-b", "feature"]);
    let query = commit(&repo, "file", b"new\n", "Feature");
    git(repo.dir.path(), ["checkout", "main"]);
    commit(&repo, "unrelated", b"one\n", "Unrelated mainline");
    git(
        repo.dir.path(),
        ["merge", "--no-ff", "feature", "-m", "Merge"],
    );
    let merge = repo.head();
    let value = search(&repo, &query, &[]);
    assert_eq!(value["matched_count"], 1);
    assert_eq!(value["matches"][0]["commit_id"], merge);
    assert_eq!(value["matches"][0]["comparison_basis"], "first_parent");
    git(repo.dir.path(), ["checkout", "-b", "integration", &root]);
    commit(&repo, "unrelated", b"one\n", "Integration base");
    git(
        repo.dir.path(),
        ["merge", "--no-ff", "--no-commit", "feature"],
    );
    commit(&repo, "unrelated", b"extra\n", "Unique merge integration");
    assert_eq!(search(&repo, &query, &[])["matched_count"], 1);
    git(repo.dir.path(), ["checkout", "-b", "remote-only", &root]);
    git(repo.dir.path(), ["cherry-pick", &query]);
    let remote = repo.head();
    git(
        repo.dir.path(),
        ["update-ref", "refs/remotes/origin/release", &remote],
    );
    git(repo.dir.path(), ["checkout", "main"]);
    git(repo.dir.path(), ["branch", "-D", "remote-only"]);
    let value = search(&repo, &query, &[]);
    assert_eq!(value["matched_count"], 2);
    assert!(
        value["matches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["commit_id"] == remote)
    );
    git(repo.dir.path(), ["commit", "--allow-empty", "-m", "Empty"]);
    let value = search(&repo, "HEAD", &[]);
    assert_eq!(value["query"]["integrity"], "empty");
    assert_eq!(value["matched_count"], 0);
}

#[test]
fn matches_renames_and_modes_without_losing_operation_material() {
    let repo = TestRepo::new();
    let base = commit(&repo, "file", b"same\n", "Base");
    git(repo.dir.path(), ["mv", "file", "renamed"]);
    git(repo.dir.path(), ["commit", "-m", "Rename query"]);
    let query = repo.head();
    repo.index();
    git(repo.dir.path(), ["checkout", "-b", "rename", &base]);
    git(repo.dir.path(), ["mv", "file", "renamed"]);
    git(repo.dir.path(), ["commit", "-m", "Reapply rename"]);
    let value = search(&repo, &query, &[]);
    assert_eq!(value["matched_count"], 1);
    assert_eq!(value["matches"][0]["files"][0]["operation"], "R");
    assert_eq!(value["matches"][0]["files"][0]["old_mode"], "100644");
    git(repo.dir.path(), ["checkout", "main"]);
    git(repo.dir.path(), ["update-index", "--chmod=+x", "renamed"]);
    git(repo.dir.path(), ["commit", "-m", "Executable query"]);
    let mode_query = repo.head();
    git(repo.dir.path(), ["checkout", "rename"]);
    git(repo.dir.path(), ["update-index", "--chmod=+x", "renamed"]);
    git(repo.dir.path(), ["commit", "-m", "Reapply mode"]);
    let value = search(&repo, &mode_query, &[]);
    assert_eq!(value["matched_count"], 1);
    assert_eq!(value["matches"][0]["files"][0]["new_mode"], "100755");
}
