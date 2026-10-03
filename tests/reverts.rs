mod support;

use std::{fs, process::Output};

use support::{TestRepo, git, git_command};

fn commit(repo: &TestRepo, contents: &[u8], subject: &str, date: &str) -> String {
    let path = repo.dir.path().join("src/x.rs");
    fs::create_dir_all(path.parent().unwrap()).expect("create source directory");
    fs::write(path, contents).expect("write source file");
    git(repo.dir.path(), ["add", "src/x.rs"]);
    let output = git_command(repo.dir.path())
        .args(["commit", "-m", subject])
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .expect("commit source change");
    assert!(output.status.success(), "{}", stderr(&output));
    repo.head()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn parallel_branch_revert_does_not_misclassify_candidate() {
    let repo = TestRepo::new();
    commit(&repo, b"base\n", "base", "2001-01-01T00:00:00Z");
    let unrelated = commit(
        &repo,
        b"unrelated change\n",
        "Unrelated branch change",
        "2001-01-02T00:00:00Z",
    );

    git(repo.dir.path(), ["switch", "-c", "candidate"]);
    let candidate = commit(
        &repo,
        b"candidate approach\n",
        "Try candidate approach",
        "2001-01-03T00:00:00Z",
    );

    git(
        repo.dir.path(),
        ["switch", "-c", "other", unrelated.as_str()],
    );
    let revert = git_command(repo.dir.path())
        .args(["revert", "--no-edit", unrelated.as_str()])
        .env("GIT_AUTHOR_DATE", "2001-01-04T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2001-01-04T00:00:00Z")
        .output()
        .expect("revert unrelated change");
    assert!(revert.status.success(), "{}", stderr(&revert));
    let revert = repo.head();

    git(repo.dir.path(), ["switch", "main"]);
    git(repo.dir.path(), ["merge", "--ff-only", "candidate"]);
    git(
        repo.dir.path(),
        [
            "merge",
            "--no-ff",
            "-s",
            "ours",
            "other",
            "-m",
            "merge other",
        ],
    );

    let candidate_is_revert_ancestor = git_command(repo.dir.path())
        .args([
            "merge-base",
            "--is-ancestor",
            candidate.as_str(),
            revert.as_str(),
        ])
        .status()
        .expect("check candidate/revert ancestry")
        .success();
    assert!(
        !candidate_is_revert_ancestor,
        "fixture branches must be parallel"
    );

    repo.index();
    // Keep the reverted change before the candidate and leave no path touch between the
    // candidate and revert, so the fixture exercises ancestry rather than touch suppression.
    let cache =
        rusqlite::Connection::open(repo.cache_dir().join("cache.sqlite")).expect("open test cache");
    let position = |oid: &str| {
        cache
            .query_row(
                "SELECT position FROM commits WHERE oid = ?1",
                [oid],
                |row| row.get::<_, i64>(0),
            )
            .expect("read cache position")
    };
    let unrelated_position = position(&unrelated);
    let candidate_position = position(&candidate);
    let revert_position = position(&revert);
    assert!(
        unrelated_position < candidate_position && candidate_position < revert_position,
        "unexpected cache order: unrelated={unrelated_position}, candidate={candidate_position}, revert={revert_position}"
    );
    let intervening_touches: i64 = cache
        .query_row(
            "SELECT COUNT(DISTINCT c.commit_id) FROM commits c \
             JOIN commit_paths cp ON cp.commit_id = c.commit_id \
             WHERE c.position > ?1 AND c.position < ?2 AND cp.raw_path = ?3",
            rusqlite::params![candidate_position, revert_position, b"src/x.rs".as_slice()],
            |row| row.get(0),
        )
        .expect("count intervening path changes");
    assert_eq!(intervening_touches, 0, "path touch masked the fixture");

    let examples = repo.run(["examples", "candidate", "approach"]);
    assert_eq!(examples.status.code(), Some(0), "{}", stderr(&examples));
    let examples_text = stdout(&examples);
    assert!(
        examples_text.contains("Try candidate approach"),
        "{examples_text}"
    );
    assert!(
        !examples_text.contains("demoted: later reverted"),
        "parallel-branch revert incorrectly demoted candidate:\n{examples_text}"
    );

    let failures = repo.run(["failures", "candidate", "approach"]);
    assert_eq!(failures.status.code(), Some(0), "{}", stderr(&failures));
    let failures_text = stdout(&failures);
    assert!(
        !failures_text.contains("(reverts this change)"),
        "parallel-branch revert was incorrectly linked:\n{failures_text}"
    );
}
