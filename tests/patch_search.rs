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
