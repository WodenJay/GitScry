mod support;

use std::{fs, path::Path};

use support::{TestRepo, git, git_command};

impl TestRepo {
    fn commit(&self, path: &str, contents: &[u8], subject: &str, body: Option<&str>) {
        if let Some(parent) = Path::new(path).parent() {
            fs::create_dir_all(self.dir.path().join(parent)).expect("create parent directory");
        }
        fs::write(self.dir.path().join(path), contents).expect("write tracked file");
        git(self.dir.path(), ["add", path]);
        git(self.dir.path(), ["commit", "-m", subject]);
        if let Some(body) = body {
            git(
                self.dir.path(),
                ["commit", "--amend", "-m", subject, "-m", body],
            );
        }
    }
    fn commit_files(&self, files: &[(&str, &[u8])], subject: &str) {
        for (path, contents) in files {
            if let Some(parent) = Path::new(path).parent() {
                fs::create_dir_all(self.dir.path().join(parent)).expect("create parent directory");
            }
            fs::write(self.dir.path().join(path), contents).expect("write tracked file");
        }
        git(self.dir.path(), ["add", "-A"]);
        git(self.dir.path(), ["commit", "-m", subject]);
    }

    fn remove_blob(&self, revision: &str, path: &str) {
        let spec = format!("{revision}:{path}");
        let output = git_command(self.dir.path())
            .args(["rev-parse", &spec])
            .output()
            .expect("resolve blob");
        assert!(output.status.success());
        let oid = String::from_utf8(output.stdout).expect("blob oid is utf-8");
        let oid = oid.trim();
        fs::remove_file(
            self.dir
                .path()
                .join(".git/objects")
                .join(&oid[..2])
                .join(&oid[2..]),
        )
        .expect("remove loose blob");
    }
    fn remove_commit(&self, revision: &str) {
        let output = git_command(self.dir.path())
            .args(["rev-parse", revision])
            .output()
            .expect("resolve commit");
        assert!(output.status.success());
        let oid = String::from_utf8(output.stdout).expect("commit oid is utf-8");
        let oid = oid.trim();
        fs::remove_file(
            self.dir
                .path()
                .join(".git/objects")
                .join(&oid[..2])
                .join(&oid[2..]),
        )
        .expect("remove loose commit");
    }
}

#[test]
fn trace_fix_reports_the_introducing_change_and_fix_as_one_chain() {
    let repo = TestRepo::new();
    repo.commit("app.txt", b"safe\n", "Initial app", None);
    repo.commit("app.txt", b"buggy\n", "Introduce bug", None);
    repo.commit(
        "app.txt",
        b"fixed\n",
        "Fix observed failure #19",
        Some("The old behavior failed in the production test."),
    );
    repo.index();

    let output = repo.run(["trace-fix", "HEAD", "--path", "./app.txt", "--limit", "1"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Fix lineage (1 match):"));
    assert!(stdout.contains("Introduce bug"));
    assert!(stdout.contains("Fix observed failure #19"));
    assert!(stdout.contains("observed failure and fix"));
    assert!(stdout.contains("fix message references: #19"));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Remote context unavailable from local history.")
    );
}

#[test]
fn trace_fix_keeps_failure_context_on_the_blamed_path() {
    let repo = TestRepo::new();
    repo.commit_files(
        &[("a.txt", b"safe-a\n"), ("b.txt", b"safe-b\n")],
        "Initial app",
    );
    repo.commit("a.txt", b"buggy-a\n", "Introduce a bug", None);
    repo.commit("b.txt", b"reported-b\n", "Report b failure #19", None);
    repo.commit_files(
        &[("a.txt", b"fixed-a\n"), ("b.txt", b"fixed-b\n")],
        "Fix #19",
    );
    repo.index();

    let output = repo.run(["trace-fix", "HEAD", "--limit", "10"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let a_start = stdout
        .find("Introduce a bug")
        .expect("a introducing change");
    let a_block = &stdout[a_start..];
    let a_end = a_block.find("\n- ").unwrap_or(a_block.len());
    assert!(!a_block[..a_end].contains("Report b failure"));
    assert!(stdout.contains("Report b failure #19"));
}

#[test]
fn trace_fix_limit_truncates_introducing_chains() {
    let repo = TestRepo::new();
    repo.commit("app.txt", b"a\nb\n", "Initial app", None);
    repo.commit("app.txt", b"x\nb\n", "Introduce first change", None);
    repo.commit("app.txt", b"x\ny\n", "Introduce second change", None);
    repo.commit("app.txt", b"fixed\nfixed\n", "Fix both changes", None);
    repo.index();

    let output = repo.run(["trace-fix", "HEAD", "--path", "app.txt", "--limit", "1"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Fix lineage (2 matches):"));
    assert!(stdout.contains("Showing 1 of 2 matching commits; results truncated."));
}

#[test]
fn trace_fix_uses_the_first_parent_of_a_merge_fix() {
    let repo = TestRepo::new();
    repo.commit("app.txt", b"safe\n", "Initial app", None);
    repo.commit("app.txt", b"buggy\n", "Introduce bug", None);
    git(repo.dir.path(), ["checkout", "-b", "fix-branch"]);
    repo.commit("app.txt", b"fixed\n", "Fix on branch", None);
    git(repo.dir.path(), ["checkout", "main"]);
    repo.commit("other.txt", b"other\n", "Advance first parent", None);
    git(
        repo.dir.path(),
        ["merge", "--no-ff", "fix-branch", "-m", "Merge fix branch"],
    );
    repo.index();

    let output = repo.run(["trace-fix", "HEAD", "--path", "app.txt"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("Introduce bug"));
    assert!(stderr.contains("first-parent deleted lines are used"));
    assert!(stdout.contains("confidence: low"));
    assert!(stderr.contains("fix is a merge; first-parent deleted lines are used"));
}

#[test]
fn trace_fix_follows_renamed_code_before_the_fix() {
    let repo = TestRepo::new();
    repo.commit("old.txt", b"safe\n", "Initial app", None);
    repo.commit("old.txt", b"buggy\n", "Introduce bug", None);
    git(repo.dir.path(), ["mv", "old.txt", "new.txt"]);
    git(repo.dir.path(), ["commit", "-m", "Move app code"]);
    repo.commit("new.txt", b"fixed\n", "Fix moved app", None);
    repo.index();

    let output = repo.run(["trace-fix", "HEAD", "--path", "new.txt"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("path: old.txt"));
    assert!(stdout.contains("path: new.txt"));
    assert!(stdout.contains("code movement: anchored path history"));
    assert!(stdout.contains("Move app code"));
}

#[test]
fn trace_fix_rejects_an_explicit_fix_outside_the_default_cache() {
    let repo = TestRepo::new();
    repo.commit("app.txt", b"safe\n", "Initial app", None);
    repo.commit("app.txt", b"buggy\n", "Introduce bug", None);
    repo.index();
    git(repo.dir.path(), ["checkout", "-b", "fix-branch"]);
    repo.commit(
        "app.txt",
        b"fixed\n",
        "Fix branch failure",
        Some("The bug failed because the old behavior regressed."),
    );

    let output = repo.run(["trace-fix", "HEAD", "--path", "app.txt"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("outside the published cache generation")
    );
}

#[test]
fn trace_fix_scopes_paths_and_movement_to_the_blame_path() {
    let repo = TestRepo::new();
    repo.commit_files(
        &[("app.txt", b"safe\n"), ("unrelated.txt", b"old\n")],
        "Initial app",
    );
    repo.commit("app.txt", b"buggy\n", "Introduce bug", None);
    git(repo.dir.path(), ["mv", "unrelated.txt", "renamed.txt"]);
    git(repo.dir.path(), ["add", "app.txt"]);
    git(repo.dir.path(), ["commit", "-m", "Move unrelated file"]);
    repo.commit("app.txt", b"fixed\n", "Fix app", None);
    repo.index();

    let output = repo.run(["trace-fix", "HEAD", "--path", "app.txt"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("path: app.txt"));
    assert!(!stdout.contains("unrelated.txt"));
    assert!(!stdout.contains("code movement: anchored path history"));
}

#[test]
fn trace_fix_ignore_revs_warning_does_not_lower_unrelated_confidence() {
    let repo = TestRepo::new();
    repo.commit("app.txt", b"safe\n", "Initial app", None);
    let initial = repo.head();
    repo.commit("app.txt", b"buggy\n", "Introduce bug", None);
    repo.commit(
        "app.txt",
        b"fixed\n",
        "Fix app",
        Some("The bug failed in production because behavior regressed."),
    );
    fs::write(
        repo.dir.path().join(".git-blame-ignore-revs"),
        format!("{initial}\n"),
    )
    .expect("write blame ignore file");
    repo.index();

    let output = repo.run(["trace-fix", "HEAD", "--path", "app.txt"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("confidence: high"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("blame ignored revisions"));
}

#[test]
fn trace_fix_uses_only_pre_fix_failure_context_and_does_not_invent_one() {
    let repo = TestRepo::new();
    repo.commit("app.txt", b"safe\n", "Initial app", None);
    repo.commit("app.txt", b"buggy\n", "Introduce bug", None);
    repo.commit(
        "app.txt",
        b"buggy\nreported\n",
        "Report observed failure #19",
        None,
    );
    repo.commit("app.txt", b"fixed\nreported\n", "Fix #19", None);
    let fix = repo.head();
    repo.commit("app.txt", b"fixed\npost-fix\n", "Post-fix note #19", None);
    repo.index();

    let output = repo.run(["trace-fix", &fix, "--path", "app.txt"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Report observed failure #19"));
    assert!(!stdout.contains("Post-fix note #19"));
    assert!(!stdout.contains("records observed failure context"));
}

#[test]
fn trace_fix_keeps_direct_fix_facts_without_a_default_branch() {
    let repo = TestRepo::new();
    repo.commit("app.txt", b"safe\n", "Initial app", None);
    repo.commit("app.txt", b"buggy\n", "Introduce bug", None);
    repo.commit("app.txt", b"fixed\n", "Fix without default branch", None);
    let fix = repo.head();
    repo.index();
    git(repo.dir.path(), ["branch", "-m", "trunk"]);

    let output = repo.run(["trace-fix", &fix, "--path", "app.txt"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Fix without default branch"));
    assert!(stdout.contains("confidence: medium"));
}

#[test]
fn trace_fix_degrades_when_the_fix_parent_commit_is_missing() {
    let repo = TestRepo::new();
    repo.commit("app.txt", b"safe\n", "Initial app", None);
    repo.commit("app.txt", b"buggy\n", "Introduce bug", None);
    let missing_parent = repo.head();
    repo.commit("app.txt", b"fixed\n", "Fix app", None);
    let fix = repo.head();
    repo.index();
    repo.remove_commit(&missing_parent);

    let output = repo.run(["trace-fix", &fix, "--path", "app.txt"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "No introducing change could be traced.",
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("fix parent is missing locally"));
    assert!(stderr.contains("changed-path metadata is unavailable"));
}

#[test]
fn trace_fix_reports_shallow_parent_degradation() {
    let repo = TestRepo::new();
    repo.commit("app.txt", b"safe\n", "Initial app", None);
    repo.commit("app.txt", b"buggy\n", "Introduce bug", None);
    repo.commit("app.txt", b"fixed\n", "Fix app", None);
    let clone_holder = tempfile::tempdir().expect("create shallow clone holder");
    git(
        clone_holder.path(),
        [
            "clone",
            "--depth",
            "1",
            "--no-local",
            repo.dir.path().to_str().unwrap(),
            "clone",
        ],
    );
    let clone = clone_holder.path().join("clone");

    let indexed = TestRepo::run_at(&clone, ["index"]);
    assert!(
        indexed.status.success(),
        "index failed: {}",
        String::from_utf8_lossy(&indexed.stderr)
    );

    let output = TestRepo::run_at(&clone, ["trace-fix", "HEAD", "--path", "app.txt"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "No introducing change could be traced.",
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("fix parent is missing locally"));
}

#[test]
fn trace_fix_reports_missing_blob_degradation() {
    let repo = TestRepo::new();
    repo.commit("app.txt", b"safe\n", "Initial app", None);
    repo.commit("app.txt", b"buggy\n", "Introduce bug", None);
    repo.commit("app.txt", b"fixed\n", "Fix app", None);
    repo.index();
    repo.remove_blob("HEAD^", "app.txt");

    let output = repo.run(["trace-fix", "HEAD", "--path", "app.txt"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "No introducing change could be traced.",
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("missing locally"));
    assert!(stderr.contains("diff hunks are unavailable"));
}

#[test]
fn trace_fix_degrades_pure_additions_without_fabricating_lineage() {
    let repo = TestRepo::new();
    repo.commit("app.txt", b"safe\n", "Initial app", None);
    repo.commit("app.txt", b"safe\nnew\n", "Add behavior", None);
    repo.index();

    let output = repo.run(["trace-fix", "HEAD", "--path", "app.txt"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "No introducing change could be traced."
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("no deleted lines"));
}
