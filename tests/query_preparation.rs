mod support;

use std::fs;
use support::{TestRepo, git};

fn target_queries(revision: &str) -> Vec<Vec<&str>> {
    vec![
        vec!["why", "file.rs", "--line", "1", "--at", revision, "--json"],
        vec!["timeline", "file.rs", "--at", revision, "--json"],
        vec![
            "regression",
            "broken",
            "--path",
            "file.rs",
            "--bad",
            revision,
            "--json",
        ],
        vec!["hotspots", "--to-rev", revision, "--json"],
        vec!["trace-fix", revision, "--json"],
    ]
}

fn commit(repo: &TestRepo, contents: &str) {
    fs::write(repo.dir.path().join("file.rs"), contents).unwrap();
    git(repo.dir.path(), ["add", "file.rs"]);
    git(repo.dir.path(), ["commit", "-m", "Update file"]);
}

#[test]
fn target_queries_require_explicit_cache_initialization() {
    let repo = TestRepo::new();
    commit(&repo, "fn example() {}\n");
    for args in target_queries("HEAD") {
        let output = repo.run(&args);
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{args:?}");
        assert!(
            error.contains("no published cache found"),
            "{args:?}: {error}"
        );
    }
}

#[test]
fn explicit_targets_do_not_authorize_refreshing_unrelated_history() {
    let repo = TestRepo::new();
    commit(&repo, "fn example() {}\n");
    repo.index();
    git(repo.dir.path(), ["checkout", "--orphan", "unrelated"]);
    commit(&repo, "fn example() { unrelated(); }\n");
    let unrelated = repo.head();
    git(repo.dir.path(), ["checkout", "main"]);
    // Each query must still reject the target: no earlier query may quietly
    // publish unrelated history and make a later query appear valid.
    for args in target_queries(&unrelated) {
        let output = repo.run(&args);
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{args:?}");
        assert!(
            error.contains("outside the published cache generation"),
            "{args:?}: {error}"
        );
    }
}
