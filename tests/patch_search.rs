mod support;

use serde_json::Value;
use std::fs;
use support::{TestRepo, git, git_command, git_stdout};

fn commit(repo: &TestRepo, path: &str, content: &[u8], message: &str) -> String {
    fs::write(repo.dir.path().join(path), content).unwrap();
    git(repo.dir.path(), ["add", path]);
    git(repo.dir.path(), ["commit", "-m", message]);
    repo.head()
}

fn search(repo: &TestRepo, revision: &str, extra: &[&str]) -> Value {
    search_with_relation(repo, revision, Some("equivalent"), extra)
}

fn search_with_relation(
    repo: &TestRepo,
    revision: &str,
    relation: Option<&str>,
    extra: &[&str],
) -> Value {
    let mut args = vec!["search", "--patch-of", revision];
    if let Some(relation) = relation {
        args.extend(["--relation", relation]);
    }
    args.push("--json");
    args.extend_from_slice(extra);
    let output = TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
        .args(args)
        .env("GITSCRY_FULL_OUTPUT", "1")
        .output()
        .unwrap();
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
fn patch_input_rejects_partial_paths_other_modes_and_unsupported_relations() {
    let repo = TestRepo::new();
    for extra in [
        vec!["words"],
        vec!["--code", "text"],
        vec!["--code-regex", "text"],
        vec!["--code-file", "file"],
        vec!["--path", "file"],
        vec!["--hybrid"],
        vec!["--github-links"],
        vec!["--relation", "unknown"],
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

#[test]
fn backfills_invalidates_and_extends_derived_cache_without_changing_material() {
    let repo = TestRepo::new();
    let base = commit(&repo, "file", b"old\n", "Base");
    let query = commit(&repo, "file", b"new\n", "Query");
    repo.index(); // A pre-feature history cache has no patch fingerprints.
    git(repo.dir.path(), ["checkout", "-b", "release", &base]);
    commit(&repo, "file", b"new\n", "Reapplication");
    let original = search(&repo, &query, &[]);
    let derived = repo.cache_dir().join("patch-relationships.sqlite");
    let connection = rusqlite::Connection::open(&derived).unwrap();
    connection.execute_batch("PRAGMA user_version=999").unwrap();
    drop(connection);
    assert_eq!(search(&repo, &query, &[]), original);
    fs::remove_file(derived).unwrap();
    assert_eq!(search(&repo, &query, &[]), original);
    git(repo.dir.path(), ["checkout", "-b", "next-release", &base]);
    commit(&repo, "file", b"new\n", "New history");
    assert_eq!(search(&repo, &query, &[])["matched_count"], 2);
    assert_eq!(
        search(&repo, &query, &["--from-rev", &base])["matched_count"],
        2
    );
    assert_eq!(
        search(&repo, &query, &["--since", "2100-01-01"])["matched_count"],
        0
    );
}

#[test]
fn missing_candidate_blobs_are_reverified_and_repaired_after_warm_queries() {
    let repo = TestRepo::new();
    let base = commit(&repo, "file", b"old\n", "Base");
    let query = commit(&repo, "file", b"new\n", "Query");
    repo.index();
    git(repo.dir.path(), ["checkout", "-b", "shifted", &base]);
    commit(&repo, "file", b"context\nold\n", "Shift base");
    let equivalent = commit(&repo, "file", b"context\nnew\n", "Shifted equivalent");
    let original = search(&repo, &query, &[]);
    assert_eq!(original["matched_count"], 1);
    let blob = repo.head_oid("HEAD:file");
    let blob_path = repo
        .common_dir()
        .join("objects")
        .join(&blob[..2])
        .join(&blob[2..]);
    let saved = fs::read(&blob_path).unwrap();
    git(repo.dir.path(), ["checkout", "main"]);
    fs::remove_file(&blob_path).unwrap();
    let value = search(&repo, &query, &[]);
    assert_eq!(value["matched_count"], 0, "{value:#}");
    assert_eq!(value["scope"]["coverage_complete"], false);
    assert!(
        value["scope"]["indeterminate"]
            .as_array()
            .unwrap()
            .iter()
            .any(|gap| gap["commit_id"] == equivalent)
    );
    fs::write(blob_path, saved).unwrap();
    assert_eq!(search(&repo, &query, &[]), original);
}

#[test]
fn warm_nonmatching_fingerprints_disclose_missing_material() {
    let repo = TestRepo::new();
    let base = commit(&repo, "file", b"old\n", "Base");
    let query = commit(&repo, "file", b"new\n", "Query");
    repo.index();
    git(repo.dir.path(), ["checkout", "-b", "different", &base]);
    let different = commit(&repo, "file", b"different\n", "Nonmatch");
    let blob = repo.head_oid("HEAD:file");
    let original = search(&repo, &query, &[]);
    assert_eq!(original["matched_count"], 0);
    assert_eq!(original["scope"]["coverage_complete"], true);
    git(repo.dir.path(), ["checkout", "main"]);
    let path = repo
        .common_dir()
        .join("objects")
        .join(&blob[..2])
        .join(&blob[2..]);
    let saved = fs::read(&path).unwrap();
    fs::remove_file(&path).unwrap();
    let value = search(&repo, &query, &[]);
    assert_eq!(value["matched_count"], 0);
    assert_eq!(value["scope"]["coverage_complete"], false);
    assert!(
        value["scope"]["indeterminate"]
            .as_array()
            .unwrap()
            .iter()
            .any(|gap| gap["commit_id"] == different)
    );
    fs::write(path, saved).unwrap();
    assert_eq!(search(&repo, &query, &[]), original);
}

#[test]
fn human_patch_paths_use_git_quoting() {
    let repo = TestRepo::new();
    let base = commit(&repo, "my file", b"old\n", "Base");
    let query = commit(&repo, "my file", b"new\n", "Query");
    repo.index();
    git(repo.dir.path(), ["checkout", "-b", "release", &base]);
    commit(&repo, "my file", b"new\n", "Reapplication");
    let output = repo.run(["search", "--patch-of", &query]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("  \"my\\040file\"\n"), "{text}");
}

#[test]
fn full_patch_beyond_display_limits_is_compared_and_shallow_scope_is_disclosed() {
    let repo = TestRepo::new();
    let base = commit(&repo, "file", b"old\n", "Base");
    let large: Vec<u8> = (0..12000)
        .flat_map(|i| format!("new line {i}\n").into_bytes())
        .collect();
    let query = commit(&repo, "file", &large, "Large query");
    repo.index();
    git(repo.dir.path(), ["checkout", "-b", "release", &base]);
    let equivalent = commit(&repo, "file", &large, "Large equivalent");
    assert_eq!(search(&repo, &query, &[])["matched_count"], 1);
    let mut extra = large.clone();
    extra.extend_from_slice(b"extra modification\n");
    commit(&repo, "file", &extra, "Extra change");
    assert_eq!(search(&repo, &query, &[])["matched_count"], 1);
    fs::write(repo.common_dir().join("shallow"), format!("{base}\n")).unwrap();
    let value = search(&repo, &query, &[]);
    assert_eq!(value["scope"]["coverage_complete"], false);
    assert_eq!(value["matches"][0]["commit_id"], equivalent);
}

#[test]
fn finds_reverts_manual_inverse_patches_and_reintroductions_separately() {
    let repo = TestRepo::new();
    let old = b"prefix\nold one\nold two\nsuffix\n";
    let new = b"prefix\nnew one\nnew two\nsuffix\n";
    commit(&repo, "file", old, "Base");
    let query = commit(&repo, "file", new, "Forward patch");
    let query_time = git_stdout(repo.dir.path(), ["show", "-s", "--format=%ct", &query])
        .parse::<i64>()
        .unwrap();
    repo.index();

    git(repo.dir.path(), ["revert", "--no-edit", &query]);
    let reverted = repo.head();
    git(repo.dir.path(), ["revert", "--no-edit", &reverted]);
    let reintroduced = repo.head();

    git(repo.dir.path(), ["checkout", "--orphan", "independent"]);
    git(repo.dir.path(), ["rm", "-rf", "."]);
    fs::write(repo.dir.path().join("file"), new).unwrap();
    git(repo.dir.path(), ["add", "file"]);
    git(repo.dir.path(), ["commit", "-m", "Independent baseline"]);
    fs::write(repo.dir.path().join("file"), old).unwrap();
    git(repo.dir.path(), ["add", "file"]);
    let output = git_command(repo.dir.path())
        .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z")
        .args(["commit", "-m", "Restore earlier content"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let manual_inverse = repo.head();
    let inverse_time = git_stdout(
        repo.dir.path(),
        ["show", "-s", "--format=%ct", &manual_inverse],
    )
    .parse::<i64>()
    .unwrap();
    assert!(inverse_time < query_time);
    git(repo.dir.path(), ["checkout", "main"]);

    let all = search_with_relation(&repo, &query, None, &[]);
    assert_eq!(all["matched_count"], 3);
    assert_eq!(all["scope"]["coverage_complete"], true);
    let matches = all["matches"].as_array().unwrap();
    for (relation, oid) in [
        ("inverse", &reverted),
        ("inverse", &manual_inverse),
        ("equivalent", &reintroduced),
    ] {
        let material = matches
            .iter()
            .find(|material| material["commit_id"] == *oid)
            .unwrap();
        assert_eq!(material["relation"], relation);
        assert_eq!(material["comparison_basis"], "first_parent");
        assert_eq!(
            material["files"][0]["old_path"],
            material["files"][0]["new_path"]
        );
    }

    let inverse = search_with_relation(&repo, &query, Some("inverse"), &[]);
    assert_eq!(inverse["matched_count"], 2);
    assert!(
        inverse["matches"]
            .as_array()
            .unwrap()
            .iter()
            .all(|material| {
                material["relation"] == "inverse" && material["commit_id"] != reintroduced
            })
    );
    let text = repo.run(["search", "--patch-of", &query]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(
        text.contains("equivalent") && text.contains("inverse"),
        "{text}"
    );
}

#[test]
fn rejects_partial_and_superficially_similar_inverse_patches() {
    let repo = TestRepo::new();
    let base = commit(&repo, "file", b"old one\nold two\n", "Base");
    let query = commit(&repo, "file", b"new one\nnew two\n", "Forward patch");
    repo.index();

    git(repo.dir.path(), ["checkout", "-b", "partial", &query]);
    commit(&repo, "file", b"old one\nnew two\n", "Partial reversal");
    git(repo.dir.path(), ["checkout", "-b", "similar", &query]);
    commit(&repo, "file", b"newer one\nnew two\n", "Similar text");
    git(repo.dir.path(), ["checkout", "main"]);
    let inverse = search_with_relation(&repo, &query, Some("inverse"), &[]);
    assert_eq!(inverse["matched_count"], 0);
    assert_eq!(inverse["scope"]["coverage_complete"], true);
    assert_eq!(base, repo.head_oid("HEAD~1"));
}

#[test]
fn missing_inverse_patch_objects_are_reported_as_indeterminate() {
    let repo = TestRepo::new();
    let old = b"old\n";
    let new = b"new\n";
    commit(&repo, "file", old, "Base");
    let query = commit(&repo, "file", new, "Forward patch");
    repo.index();

    git(repo.dir.path(), ["checkout", "--orphan", "independent"]);
    git(repo.dir.path(), ["rm", "-rf", "."]);
    fs::write(repo.dir.path().join("file"), new).unwrap();
    fs::write(repo.dir.path().join("context"), b"branch-only context\n").unwrap();
    git(repo.dir.path(), ["add", "."]);
    git(repo.dir.path(), ["commit", "-m", "Independent baseline"]);
    fs::write(repo.dir.path().join("file"), old).unwrap();
    git(repo.dir.path(), ["add", "file"]);
    git(repo.dir.path(), ["commit", "-m", "Manual inverse"]);
    let inverse_oid = repo.head();
    git(repo.dir.path(), ["checkout", "main"]);

    let complete = search_with_relation(&repo, &query, Some("inverse"), &[]);
    assert_eq!(complete["matched_count"], 1);
    assert_eq!(complete["matches"][0]["commit_id"], inverse_oid);

    let tree = repo.head_oid(&format!("{inverse_oid}^{{tree}}"));
    let object_path = repo
        .common_dir()
        .join("objects")
        .join(&tree[..2])
        .join(&tree[2..]);
    let saved = fs::read(&object_path).unwrap();
    fs::remove_file(&object_path).unwrap();
    let incomplete = search_with_relation(&repo, &query, Some("inverse"), &[]);
    assert_eq!(incomplete["matched_count"], 0);
    assert_eq!(incomplete["scope"]["coverage_complete"], false);
    assert!(
        incomplete["scope"]["indeterminate"]
            .as_array()
            .unwrap()
            .iter()
            .any(|gap| gap["commit_id"] == inverse_oid)
    );
    fs::write(object_path, saved).unwrap();
    assert_eq!(
        search_with_relation(&repo, &query, Some("inverse"), &[]),
        complete
    );
}

#[test]
fn finds_complete_inverse_across_file_operations_and_no_newline_edits() {
    let repo = TestRepo::new();
    let old_rename = b"first\nsecond\nthird\nfourth\nfifth\n";
    let new_rename = b"first\nsecond changed\nthird\nfourth\nfifth\n";

    fs::write(repo.dir.path().join("old-name"), old_rename).unwrap();
    fs::write(repo.dir.path().join("executable"), b"mode\n").unwrap();
    fs::write(repo.dir.path().join("deleted"), b"deleted content\n").unwrap();
    fs::write(repo.dir.path().join("no-newline"), b"old without newline").unwrap();
    git(repo.dir.path(), ["add", "."]);
    git(repo.dir.path(), ["commit", "-m", "Base"]);

    git(repo.dir.path(), ["mv", "old-name", "new-name"]);
    fs::write(repo.dir.path().join("new-name"), new_rename).unwrap();
    git(repo.dir.path(), ["add", "new-name"]);
    git(repo.dir.path(), ["rm", "deleted"]);
    fs::write(repo.dir.path().join("added"), b"added content\n").unwrap();
    git(repo.dir.path(), ["add", "added"]);
    fs::write(repo.dir.path().join("no-newline"), b"new without newline").unwrap();
    git(repo.dir.path(), ["add", "no-newline"]);
    git(
        repo.dir.path(),
        ["update-index", "--chmod=+x", "executable"],
    );
    git(repo.dir.path(), ["commit", "-m", "Forward patch"]);
    let query = repo.head();
    repo.index();

    git(repo.dir.path(), ["checkout", "--orphan", "independent"]);
    git(repo.dir.path(), ["rm", "-rf", "."]);
    fs::write(repo.dir.path().join("new-name"), new_rename).unwrap();
    fs::write(repo.dir.path().join("executable"), b"mode\n").unwrap();
    fs::write(repo.dir.path().join("added"), b"added content\n").unwrap();
    fs::write(repo.dir.path().join("no-newline"), b"new without newline").unwrap();
    git(repo.dir.path(), ["add", "."]);
    git(
        repo.dir.path(),
        ["update-index", "--chmod=+x", "executable"],
    );
    git(repo.dir.path(), ["commit", "-m", "Independent baseline"]);

    git(repo.dir.path(), ["mv", "new-name", "old-name"]);
    fs::write(repo.dir.path().join("old-name"), old_rename).unwrap();
    git(repo.dir.path(), ["add", "old-name"]);
    git(repo.dir.path(), ["rm", "added"]);
    fs::write(repo.dir.path().join("deleted"), b"deleted content\n").unwrap();
    git(repo.dir.path(), ["add", "deleted"]);
    fs::write(repo.dir.path().join("no-newline"), b"old without newline").unwrap();
    git(repo.dir.path(), ["add", "no-newline"]);
    git(
        repo.dir.path(),
        ["update-index", "--chmod=-x", "executable"],
    );
    git(repo.dir.path(), ["commit", "-m", "Manual inverse"]);
    let inverse = repo.head();
    git(repo.dir.path(), ["checkout", "main"]);

    let result = search_with_relation(&repo, &query, Some("inverse"), &[]);
    assert_eq!(result["matched_count"], 1);
    let material = &result["matches"][0];
    assert_eq!(material["commit_id"], inverse);
    assert_eq!(material["relation"], "inverse");
    assert_eq!(result["scope"]["coverage_complete"], true);

    let files = material["files"].as_array().unwrap();
    let operations: Vec<_> = files
        .iter()
        .map(|file| file["operation"].as_str().unwrap())
        .collect();
    for operation in ["A", "D", "M", "R"] {
        assert!(
            operations.contains(&operation),
            "missing {operation}: {operations:?}"
        );
    }
    assert!(
        files
            .iter()
            .any(|file| { file["old_mode"] == "100755" && file["new_mode"] == "100644" })
    );
}
