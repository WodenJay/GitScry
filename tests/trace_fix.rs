mod support;

use std::{fs, path::Path};

use support::{TestRepo, git, git_command, git_stdout};

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
fn trace_fix_batches_deleted_ranges_without_losing_per_line_attribution() {
    let repo = TestRepo::new();
    let initial = b"old_a\nold_b\nkeep\nold_c\nold_d\n";
    repo.commit("app.txt", initial, "Initial app", None);
    repo.commit(
        "app.txt",
        b"bug_a\nbug_b\nkeep\nold_c\nold_d\n",
        "Introduce first bug",
        None,
    );
    let first = repo.head();
    repo.commit(
        "app.txt",
        b"bug_a\nbug_b\nkeep\nbug_c\nbug_d\n",
        "Introduce second bug",
        None,
    );
    let second = repo.head();
    repo.commit(
        "app.txt",
        b"prefix\nbug_a\nbug_b\nkeep\nbug_c\nbug_d\n",
        "Shift original line coordinates",
        None,
    );
    repo.commit(
        "app.txt",
        b"prefix\nfixed_a\nfixed_b\nkeep\nfixed_c\nfixed_d\n",
        "Fix observed failures",
        None,
    );
    repo.index();

    let trace_file = repo.dir.path().join("blame-trace.log");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_gitscry"))
        .args([
            "trace-fix",
            "HEAD",
            "--path",
            "app.txt",
            "--patch",
            "--json",
        ])
        .current_dir(repo.dir.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", repo.dir.path().join("global-config"))
        .env("GIT_TRACE", &trace_file)
        .output()
        .expect("run trace-fix with Git tracing");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let materials = report["materials"].as_array().unwrap();
    assert_eq!(materials.len(), 2);
    for (oid, text) in [(&first, "bug_a"), (&second, "bug_c")] {
        let material = materials
            .iter()
            .find(|material| material["citations"][0]["oid"] == *oid)
            .unwrap();
        assert!(
            material["basis"]
                .as_array()
                .unwrap()
                .iter()
                .any(|basis| basis == "deleted lines: 2")
        );
        assert_eq!(material["patch"]["status"], "available");
        assert_eq!(material["patch"]["commit_oid"], *oid);
        assert!(
            material["patch"]["hunks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|hunk| hunk["text"].as_str().unwrap().contains(text))
        );
    }
    let trace = fs::read_to_string(trace_file).expect("read Git trace");
    assert_eq!(
        trace
            .lines()
            .filter(|line| line.contains("built-in: git blame "))
            .count(),
        2,
        "{trace}"
    );
    fs::write(
        repo.dir.path().join(".git-blame-ignore-revs"),
        "not-a-revision\n",
    )
    .expect("write invalid blame ignore file");
    let fallback = repo.run([
        "trace-fix",
        "HEAD",
        "--path",
        "app.txt",
        "--patch",
        "--json",
    ]);
    assert!(
        fallback.status.success(),
        "{}",
        String::from_utf8_lossy(&fallback.stderr)
    );
    let fallback: serde_json::Value = serde_json::from_slice(&fallback.stdout).unwrap();
    assert_eq!(fallback["materials"], report["materials"]);
}

#[test]
fn trace_fix_keeps_hunks_aligned_after_type_change() {
    let repo = TestRepo::new();
    repo.commit_files(
        &[
            ("a-relnotes", b"old release notes\n"),
            ("z-app.txt", b"safe\n"),
        ],
        "Initial app",
    );
    repo.commit("z-app.txt", b"buggy\n", "Introduce bug", None);
    fs::write(
        repo.dir.path().join("symlink-target"),
        b"Documentation/RelNotes/2.3.0.txt",
    )
    .expect("write symlink target blob");
    let target_blob = git_stdout(repo.dir.path(), ["hash-object", "-w", "symlink-target"]);
    fs::remove_file(repo.dir.path().join("symlink-target")).expect("remove target helper file");
    let cacheinfo = format!("120000,{target_blob},a-relnotes");
    git(
        repo.dir.path(),
        ["update-index", "--add", "--cacheinfo", cacheinfo.as_str()],
    );
    fs::write(repo.dir.path().join("z-app.txt"), b"fixed\n").expect("write fix");
    git(repo.dir.path(), ["add", "z-app.txt"]);
    git(repo.dir.path(), ["commit", "-m", "Fix #19"]);
    let fix = repo.head();
    repo.index();

    let output = repo.run(["trace-fix", &fix, "--path", "z-app.txt"]);
    assert!(
        output.status.success(),
        "trace-fix failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Introduce bug"), "{stdout}");
    assert!(stdout.contains("Fix #19"), "{stdout}");
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
fn trace_fix_patch_quotes_only_the_attributed_introducing_hunk() {
    let repo = TestRepo::new();
    let stable_tail = b"stable_01\nstable_02\nstable_03\nstable_04\nstable_05\nstable_06\nstable_07\nstable_08\nstable_09\nstable_10\nstable_11\nstable_12\n";
    let mut initial_app = b"safe\nhealthy\n".to_vec();
    initial_app.extend_from_slice(stable_tail);
    initial_app.extend_from_slice(b"initial_tail\n");
    let mut introducing_app = b"safe\nhealthy\nbuggy_impl\n".to_vec();
    introducing_app.extend_from_slice(stable_tail);
    introducing_app.extend_from_slice(b"app_hunk_noise\n");
    let mut failure_app =
        b"prefix_a\nprefix_b\nprefix_c\nprefix_d\nprefix_e\nprefix_f\nsafe\nhealthy\nbuggy_impl\nfailure_observed\n".to_vec();
    failure_app.extend_from_slice(stable_tail);
    failure_app.extend_from_slice(b"app_hunk_noise\n");
    let mut fixed_app =
        b"prefix_a\nprefix_b\nprefix_c\nprefix_d\nprefix_e\nprefix_f\nsafe\nhealthy\nfixed_impl\nfailure_observed\n".to_vec();
    fixed_app.extend_from_slice(stable_tail);
    fixed_app.extend_from_slice(b"app_hunk_noise\n");
    repo.commit_files(
        &[("app.txt", &initial_app), ("unrelated.txt", b"before\n")],
        "Initial app",
    );
    repo.commit_files(
        &[
            ("app.txt", &introducing_app),
            ("unrelated.txt", b"same_commit_noise\n"),
        ],
        "Introduce broken behavior",
    );
    let introducing = repo.head();
    repo.commit("app.txt", &failure_app, "Observe failure #19", None);
    let failure = repo.head();
    repo.commit(
        "app.txt",
        &fixed_app,
        "Fix #19",
        Some("Correct the failed implementation."),
    );
    let fix = repo.head();
    repo.index();

    let ordinary_text = repo.run(["trace-fix", &fix, "--path", "app.txt"]);
    assert_eq!(ordinary_text.status.code(), Some(0));
    assert!(!String::from_utf8_lossy(&ordinary_text.stdout).contains("patch excerpt:"));

    let ordinary_json = repo.run(["trace-fix", &fix, "--path", "app.txt", "--json"]);
    assert_eq!(ordinary_json.status.code(), Some(0));
    let ordinary_json: serde_json::Value = serde_json::from_slice(&ordinary_json.stdout).unwrap();
    assert_eq!(ordinary_json["schema_version"], 1);
    assert!(ordinary_json["materials"][0].get("patch").is_none());

    let text_patch = repo.run(["trace-fix", &fix, "--path", "app.txt", "--patch"]);
    assert_eq!(
        text_patch.status.code(),
        Some(0),
        "trace-fix --patch: {}",
        String::from_utf8_lossy(&text_patch.stderr)
    );
    let text_patch = String::from_utf8_lossy(&text_patch.stdout);
    assert!(
        text_patch.contains("patch excerpt: available"),
        "{text_patch}"
    );
    assert!(text_patch.contains("hunk: app.txt"), "{text_patch}");
    assert!(text_patch.contains("buggy_impl"), "{text_patch}");
    assert!(!text_patch.contains("app_hunk_noise"), "{text_patch}");
    assert!(!text_patch.contains("same_commit_noise"), "{text_patch}");
    assert!(!text_patch.contains("failure_observed"), "{text_patch}");
    assert!(!text_patch.contains("fixed_impl"), "{text_patch}");

    let json_patch = repo.run(["trace-fix", &fix, "--path", "app.txt", "--patch", "--json"]);
    assert_eq!(
        json_patch.status.code(),
        Some(0),
        "trace-fix --patch --json: {}",
        String::from_utf8_lossy(&json_patch.stderr)
    );
    let json_patch: serde_json::Value = serde_json::from_slice(&json_patch.stdout).unwrap();
    assert_eq!(json_patch["schema_version"], 2);
    let material = &json_patch["materials"][0];
    let citations = material["citations"].as_array().unwrap();
    assert_eq!(citations[0]["oid"], introducing);
    assert!(citations.iter().any(|citation| citation["oid"] == failure));
    assert!(citations.iter().any(|citation| citation["oid"] == fix));
    let patch = &material["patch"];
    assert_eq!(patch["commit_oid"], introducing);
    assert_eq!(patch["status"], "available");
    let hunks = patch["hunks"].as_array().unwrap();
    assert_eq!(hunks.len(), 1);
    assert!(
        hunks
            .iter()
            .all(|hunk| { hunk["old_path"] == "app.txt" || hunk["new_path"] == "app.txt" })
    );
    let patch_text = hunks
        .iter()
        .filter_map(|hunk| hunk["text"].as_str())
        .collect::<String>();
    assert!(patch_text.contains("buggy_impl"));
    assert!(!patch_text.contains("app_hunk_noise"));
    assert!(!patch_text.contains("same_commit_noise"));
    assert!(!patch_text.contains("failure_observed"));
    assert!(!patch_text.contains("fixed_impl"));
}

#[test]
fn trace_fix_patch_uses_hunk_metadata_when_cached_text_is_missing() {
    let repo = TestRepo::new();
    repo.commit("app.txt", b"safe\n", "Initial app", None);
    let mut oversized_line = vec![b'x'; 100_000];
    oversized_line.push(b'\n');
    repo.commit(
        "app.txt",
        &oversized_line,
        "Introduce oversized behavior",
        None,
    );
    repo.commit("app.txt", b"fixed\n", "Fix oversized behavior", None);
    repo.index();

    let output = repo.run([
        "trace-fix",
        "HEAD",
        "--path",
        "app.txt",
        "--patch",
        "--json",
    ]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "trace-fix --patch --json: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["schema_version"], 2);
    let patch = &report["materials"][0]["patch"];
    assert_eq!(patch["status"], "available");
    assert_eq!(patch["hunks"].as_array().unwrap().len(), 1);
    assert!(patch["hunks"][0]["text"].is_null());
    assert_eq!(patch["hunks"][0]["truncated"], true);
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

#[test]
fn trace_fix_scope_filters_material_without_retargeting_the_fix() {
    let repo = TestRepo::new();
    repo.commit("app.txt", b"safe\n", "Initial app", None);
    let initial = repo.head();
    repo.commit("app.txt", b"buggy\n", "Introduce timeout bug", None);
    let introducing = repo.head();
    repo.commit(
        "app.txt",
        b"fixed\n",
        "Fix timeout bug",
        Some("The timeout behavior is fixed."),
    );
    let fix = repo.head();
    repo.commit("later.txt", b"later\n", "Post-fix unrelated change", None);
    let cache_tip = repo.head();
    repo.index();

    let output = repo.run([
        "trace-fix",
        fix.as_str(),
        "--path",
        "app.txt",
        "--from-rev",
        initial.as_str(),
        "--to-rev",
        introducing.as_str(),
        "--since",
        "2000-01-01",
        "--until",
        "2099-12-31",
        "--patch",
        "--json",
    ]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["scope"]["from_rev"], initial);
    assert_eq!(report["scope"]["to_rev"], introducing);
    assert_eq!(report["scope"]["since"], "2000-01-01");
    assert_eq!(report["scope"]["until"], "2099-12-31");
    assert_eq!(report["scope"]["cache_tip"], cache_tip);
    assert_eq!(report["materials"][0]["subject"], "Introduce timeout bug");
    assert_eq!(report["materials"][0]["detail"]["fix_revision"], fix);
    assert_eq!(report["materials"][0]["patch"]["status"], "available");
    assert!(
        report["materials"][0]["patch"]["hunks"][0]["text"]
            .as_str()
            .unwrap()
            .contains("+buggy")
    );

    let empty = repo.run([
        "trace-fix",
        fix.as_str(),
        "--path",
        "app.txt",
        "--from-rev",
        introducing.as_str(),
        "--json",
    ]);
    assert_eq!(
        empty.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&empty.stderr)
    );
    let empty_report: serde_json::Value = serde_json::from_slice(&empty.stdout).unwrap();
    assert_eq!(empty_report["matched_count"], 0);
    assert_eq!(empty_report["materials"], serde_json::json!([]));
    assert_eq!(empty_report["scope"]["to_rev"], fix);
    assert_eq!(empty_report["scope"]["cache_tip"], cache_tip);
    assert!(
        empty_report["notices"]
            .as_array()
            .unwrap()
            .iter()
            .all(|notice| {
                notice.as_str()
                    != Some(
                        "warning: no introducing commit is available in the default-branch cache.",
                    )
            }),
        "a scoped empty result must not claim the cache has no introducing commit: {empty_report}"
    );

    let invalid = repo.run([
        "trace-fix",
        fix.as_str(),
        "--path",
        "app.txt",
        "--from-rev",
        fix.as_str(),
        "--to-rev",
        introducing.as_str(),
    ]);
    assert_eq!(invalid.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&invalid.stderr)
            .contains("--from-rev must be an ancestor of --to-rev")
    );

    let human = repo.run([
        "trace-fix",
        fix.as_str(),
        "--path",
        "app.txt",
        "--from-rev",
        introducing.as_str(),
    ]);
    assert_eq!(human.status.code(), Some(0));
    let human = String::from_utf8_lossy(&human.stdout);
    assert!(human.contains("Scope: commits reachable from"));
    assert!(human.contains("No introducing change could be traced."));
}
