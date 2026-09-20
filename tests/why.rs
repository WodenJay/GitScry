mod support;

use std::{fs, path::Path, process::Command};

use rusqlite::Connection;
use support::{TestRepo, git};

impl TestRepo {
    fn commit(&self, path: &str, contents: &[u8], subject: &str, body: Option<&str>) {
        if let Some(parent) = Path::new(path).parent() {
            fs::create_dir_all(self.dir.path().join(parent)).expect("create parent directory");
        }
        fs::write(self.dir.path().join(path), contents).expect("write tracked file");
        git(self.dir.path(), ["add", path]);
        git(self.dir.path(), ["commit", "-m", subject]);
        if let Some(body) = body {
            let amend = Command::new("git")
                .args(["commit", "--amend", "-m", subject, "-m", body])
                .current_dir(self.dir.path())
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .expect("amend commit");
            assert!(amend.status.success());
        }
    }

    fn rename(&self, old: &str, new: &str, subject: &str) {
        fs::rename(self.dir.path().join(old), self.dir.path().join(new)).expect("rename file");
        git(self.dir.path(), ["add", "-A"]);
        git(self.dir.path(), ["commit", "-m", subject]);
    }
}

#[test]
fn why_reports_blame_and_explanation_for_a_line() {
    let repo = TestRepo::new();
    repo.commit(
        "target.txt",
        b"target\n",
        "Add target",
        Some("Because callers need a stable target, keep this line explicit."),
    );

    let output = repo.run(["why", "target.txt", "--line", "1", "--limit", "1"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Why history (1 match):"));
    assert!(stdout.contains("target: line 1 at "));
    assert!(stdout.contains("confidence: high"));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Remote context unavailable from local history.")
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("blame reached a local history boundary")
    );
}

#[test]
fn why_resolves_symbols_and_pins_an_explicit_revision() {
    let repo = TestRepo::new();
    repo.commit("src/lib.rs", b"fn explain() {}\n", "Initial symbol", None);
    let initial = repo.head();
    repo.commit(
        "src/lib.rs",
        b"fn explain() { println!(\"new\"); }\n",
        "Later symbol change",
        None,
    );

    let output = repo.run([
        "why",
        "src/lib.rs",
        "--symbol",
        "explain",
        "--at",
        initial.as_str(),
    ]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("target: symbol explain (line 1)"));
    assert!(stdout.contains("Initial symbol"));
    assert!(!stdout.contains("Later symbol change"));
}

#[test]
fn why_keeps_explicit_out_of_cache_targets_in_memory() {
    let repo = TestRepo::new();
    repo.commit("main.txt", b"main\n", "Main history", None);
    git(repo.dir.path(), ["switch", "-c", "feature"]);
    repo.commit("feature.txt", b"feature\n", "Feature-only history", None);
    let feature_oid = repo.head();
    git(repo.dir.path(), ["switch", "main"]);

    let output = repo.run(["why", "feature.txt", "--line", "1", "--at", "feature"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Feature-only history"));

    let cache = Connection::open(repo.dir.path().join(".gitscry/cache.sqlite")).unwrap();
    let indexed: i64 = cache
        .query_row(
            "SELECT COUNT(*) FROM commits WHERE oid = ?1",
            [&feature_oid],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(indexed, 0);
}

#[test]
fn why_exposes_rename_boundary_and_rejects_invalid_anchors() {
    let repo = TestRepo::new();
    repo.commit("old.txt", b"kept\n", "Add old path", None);
    repo.rename("old.txt", "new.txt", "Move old path");

    let output = repo.run(["why", "new.txt", "--line", "1"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("rename boundary"));

    let missing_anchor = repo.run(["why", "new.txt"]);
    assert_eq!(missing_anchor.status.code(), Some(2));
    let zero_line = repo.run(["why", "new.txt", "--line", "0"]);
    assert_eq!(zero_line.status.code(), Some(2));
}

#[test]
fn why_does_not_report_unrelated_renames_as_boundaries() {
    let repo = TestRepo::new();
    repo.commit("target.txt", b"target\n", "Create target", None);
    fs::write(repo.dir.path().join("other.txt"), b"other\n").expect("write other file");
    git(repo.dir.path(), ["add", "other.txt"]);
    git(repo.dir.path(), ["commit", "-m", "Create other file"]);
    fs::write(repo.dir.path().join("target.txt"), b"changed\n").expect("update target file");
    fs::rename(
        repo.dir.path().join("other.txt"),
        repo.dir.path().join("moved.txt"),
    )
    .expect("rename other file");
    git(repo.dir.path(), ["add", "-A"]);
    git(
        repo.dir.path(),
        ["commit", "-m", "Modify target and rename unrelated file"],
    );

    let output = repo.run(["why", "target.txt", "--line", "1"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("rename boundary"));
}

#[test]
fn why_does_not_treat_hunk_context_as_line_ownership() {
    let repo = TestRepo::new();
    repo.commit(
        "context.txt",
        b"keep\ncontext\nchanged\n",
        "Create context fixture",
        None,
    );
    repo.commit(
        "context.txt",
        b"keep\ncontext\nchanged later\n",
        "Because the third line fixes a production regression.",
        None,
    );

    let output = repo.run(["why", "context.txt", "--line", "1", "--limit", "1"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Create context fixture"));
    assert!(!stdout.contains("confidence: high"));
}

#[test]
fn why_does_not_promote_an_adjacent_line_rewrite() {
    let repo = TestRepo::new();
    repo.commit("target.txt", b"one\ntwo\nthree\n", "Create target", None);
    repo.commit(
        "target.txt",
        b"one\ntwo\nthree changed\n",
        "Rewrite third line",
        Some("Because the third line fixes a production regression."),
    );
    repo.commit(
        "target.txt",
        b"one\ntwo changed\nthree changed\n",
        "Rewrite second line",
        None,
    );

    let output = repo.run(["why", "target.txt", "--line", "2", "--limit", "10"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let third = stdout
        .split("- ")
        .find(|section| section.contains("Rewrite third line"))
        .expect("show adjacent rewrite");
    assert!(!third.contains("confidence: high"));
}

#[test]
fn why_does_not_mark_unrelated_multihunk_changes_as_direct() {
    let repo = TestRepo::new();
    let mut base = (1..=40).map(|line| format!("L{line}")).collect::<Vec<_>>();
    base[27] = "SPECIAL".to_owned();
    base[29] = "ORIGIN".to_owned();
    repo.commit(
        "f.txt",
        format!("{}\n", base.join("\n")).as_bytes(),
        "Create file",
        None,
    );
    base[27] = "SPECIAL2".to_owned();
    repo.commit(
        "f.txt",
        format!("{}\n", base.join("\n")).as_bytes(),
        "Adjust unrelated line",
        Some("Because SPECIAL2 prevents a regression, this avoids an incident."),
    );
    base.drain(1..11);
    let insert_at = base.iter().position(|line| line == "L32").unwrap() + 1;
    base.splice(insert_at..insert_at, ["INS1".to_owned(), "INS2".to_owned()]);
    repo.commit(
        "f.txt",
        format!("{}\n", base.join("\n")).as_bytes(),
        "Trim above and append a block below",
        None,
    );

    let output = repo.run(["why", "f.txt", "--line", "20", "--limit", "10"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let unrelated = stdout
        .split("- ")
        .find(|section| section.contains("Adjust unrelated line"))
        .expect("show unrelated change");
    assert!(!unrelated.contains("confidence: high"));
    assert!(!unrelated.contains("diff hunk corroborates the target line"));
}

#[test]
fn why_blames_paths_with_literal_metacharacters() {
    let repo = TestRepo::new();
    repo.commit("a1.txt", b"wrong\n", "Wrong file", None);
    repo.commit("a[1].txt", b"right\n", "Bracket file", None);

    let output = repo.run(["why", "a[1].txt", "--line", "1"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Wrong file"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("Git line attribution unavailable"));
}

#[test]
fn why_does_not_run_textconv_during_blame() {
    let repo = TestRepo::new();
    repo.commit("target.txt", b"old\n", "Create target", None);
    repo.commit("target.txt", b"new\n", "Change target", None);
    fs::write(
        repo.dir.path().join(".gitattributes"),
        b"target.txt diff=evil\n",
    )
    .expect("write attributes");
    git(repo.dir.path(), ["add", ".gitattributes"]);
    git(repo.dir.path(), ["commit", "-m", "Configure textconv"]);
    git(
        repo.dir.path(),
        [
            "config",
            "diff.evil.textconv",
            "git rev-parse --show-toplevel",
        ],
    );

    let output = repo.run(["why", "target.txt", "--line", "1"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Change target"));
    assert!(
        stdout
            .split("- ")
            .nth(1)
            .is_some_and(|section| section.contains("Change target"))
    );
}

#[test]
fn why_works_without_a_main_or_master_default_branch() {
    let repo = TestRepo::new();
    repo.commit("target.txt", b"target\n", "Create target", None);
    git(repo.dir.path(), ["branch", "-m", "trunk"]);

    let output = repo.run(["why", "target.txt", "--line", "1"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Create target"));
}

#[test]
fn why_warns_for_configured_blame_ignore_file() {
    let repo = TestRepo::new();
    repo.commit("target.txt", b"first\n", "First target", None);
    repo.commit("target.txt", b"second\n", "Ignored target change", None);
    let ignored = repo.head();
    fs::write(repo.dir.path().join("myignores"), format!("{ignored}\n"))
        .expect("write configured ignore-revs file");
    git(
        repo.dir.path(),
        ["config", "blame.ignoreRevsFile", "myignores"],
    );

    let output = repo.run(["why", "target.txt", "--line", "1"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stderr).contains("blame ignored revisions"));
}

#[test]
fn why_symbol_anchor_ignores_a_prose_mention() {
    let repo = TestRepo::new();
    repo.commit(
        "src/lib.rs",
        b"// explain() is mentioned here.\nlet value = explain();\npub fn explain() {}\n",
        "Declare symbol",
        None,
    );

    let output = repo.run(["why", "src/lib.rs", "--symbol", "explain"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("target: symbol explain (line 3)"));
}

#[test]
fn why_scores_cochanged_paths_without_confusing_renames() {
    let repo = TestRepo::new();
    repo.commit("target.txt", b"target\n", "Create target", None);
    fs::write(repo.dir.path().join("related.txt"), b"related\n").expect("write related file");
    fs::write(repo.dir.path().join("target.txt"), b"target changed\n").expect("update target file");
    git(repo.dir.path(), ["add", "target.txt", "related.txt"]);
    git(
        repo.dir.path(),
        ["commit", "-m", "Change target and related files"],
    );

    let output = repo.run(["why", "target.txt", "--line", "1"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("co-changed paths (1)"));
}

#[test]
fn why_rejects_line_one_for_an_empty_file() {
    let repo = TestRepo::new();
    repo.commit("empty.txt", b"", "Create empty file", None);

    let output = repo.run(["why", "empty.txt", "--line", "1"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn why_handles_binary_files_without_failing() {
    let repo = TestRepo::new();
    repo.commit("binary.bin", b"\0\xffbinary\0", "Create binary", None);

    let output = repo.run(["why", "binary.bin", "--line", "1"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Create binary"));
}

#[test]
fn why_degrades_cleanly_at_a_submodule_boundary() {
    let repo = TestRepo::new();
    repo.commit("seed.txt", b"seed\n", "Seed repository", None);
    let oid = repo.head();
    let cacheinfo = format!("160000,{oid},module");
    git(
        repo.dir.path(),
        ["update-index", "--add", "--cacheinfo", cacheinfo.as_str()],
    );
    git(repo.dir.path(), ["commit", "-m", "Add submodule boundary"]);

    let output = repo.run(["why", "module", "--line", "1"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"No explanatory history found.\n");
    assert!(String::from_utf8_lossy(&output.stderr).contains("target path is a submodule"));
}

#[test]
fn why_warns_for_shallow_history() {
    let source = TestRepo::new();
    source.commit("target.txt", b"target\n", "Target history", None);
    let parent = tempfile::tempdir().expect("create clone parent");
    let clone = parent.path().join("clone");
    let cloned = Command::new("git")
        .args(["clone", "--depth", "1", "--no-local"])
        .arg(source.dir.path())
        .arg(&clone)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("clone shallow repository");
    assert!(cloned.status.success());

    let output = Command::new(env!("CARGO_BIN_EXE_gitscry"))
        .args(["why", "target.txt", "--line", "1"])
        .current_dir(&clone)
        .output()
        .expect("run gitscry");
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stderr).contains("local history is shallow"));
}

#[test]
fn why_marks_merge_boundaries_and_honors_limit() {
    let repo = TestRepo::new();
    repo.commit("target.txt", b"base\n", "Create target", None);
    git(repo.dir.path(), ["switch", "-c", "feature"]);
    repo.commit("target.txt", b"feature\n", "Feature target change", None);
    git(repo.dir.path(), ["switch", "main"]);
    repo.commit("main.txt", b"main\n", "Main branch change", None);
    git(
        repo.dir.path(),
        ["merge", "--no-ff", "feature", "-m", "Merge feature target"],
    );

    let merge_output = repo.run(["why", "target.txt", "--line", "1"]);
    assert_eq!(merge_output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&merge_output.stdout).contains("merge boundary"));

    repo.commit("target.txt", b"latest\n", "Latest target change", None);
    let limited = repo.run(["why", "target.txt", "--line", "1", "--limit", "1"]);
    let stdout = String::from_utf8_lossy(&limited.stdout);
    assert!(stdout.contains("results truncated"));
}

#[test]
fn why_warns_when_blame_ignores_a_revision() {
    let repo = TestRepo::new();
    repo.commit("target.txt", b"first\n", "First target", None);
    repo.commit("target.txt", b"second\n", "Ignored target change", None);
    let ignored = repo.head();
    fs::write(
        repo.dir.path().join(".git-blame-ignore-revs"),
        format!("{ignored}\n"),
    )
    .expect("write ignore-revs file");

    let output = repo.run(["why", "target.txt", "--line", "1"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stderr).contains("blame ignored revisions"));
}
