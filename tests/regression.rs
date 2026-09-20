mod support;

use std::{fs, path::Path, process::Command};

use support::{TestRepo, git, git_stdout};

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
fn regression_reports_a_historical_suspect() {
    let repo = TestRepo::new();
    repo.commit(
        "gateway/run_busy.py",
        b"def same_chat_key_slots(key, chat_id):\n    return key.startswith(chat_id)\n",
        "Add chat key matcher",
        None,
    );
    let good = repo.head();
    repo.commit(
        "gateway/run_busy.py",
        b"def same_chat_key_slots(key, chat_id):\n    slots = key.split(\":\")\n    return slots[3] == chat_id\n",
        "Refactor Matrix stop chat key matching",
        Some("Matrix stop fails for chat ids containing a colon."),
    );

    let output = repo.run([
        "regression",
        "Matrix",
        "stop",
        "chat",
        "id",
        "colon",
        "--path",
        "gateway/run_busy.py",
        "--symbol",
        "same_chat_key_slots",
        "--good",
        good.as_str(),
        "--limit",
        "1",
    ]);

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Regression suspects"));
    assert!(stdout.contains("suspect"));
    assert!(stdout.contains("Refactor Matrix stop chat key matching"));
    assert!(stdout.contains("confidence:"));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("do not replace executable git bisect")
    );
}

#[test]
fn regression_narrows_to_symbol_and_reports_test_history() {
    let repo = TestRepo::new();
    repo.commit(
        "gateway/run_busy.py",
        b"def alpha(key):\n    return key\n\ndef beta(key):\n    return key\n",
        "Add chat matchers",
        None,
    );
    let good = repo.head();
    repo.commit(
        "gateway/run_busy.py",
        b"def alpha(key):\n    return key\n\ndef beta(key):\n    return key.strip()\n",
        "Refactor unrelated beta matcher",
        None,
    );
    fs::create_dir_all(repo.dir.path().join("tests")).expect("create test directory");
    fs::write(
        repo.dir.path().join("tests/test_run_busy.py"),
        b"def test_alpha_colon(): pass\n",
    )
    .expect("write test history");
    fs::write(
        repo.dir.path().join("gateway/run_busy.py"),
        b"def alpha(key):\n    return key.replace(\":\", \"/\")\n\ndef beta(key):\n    return key.strip()\n",
    )
    .expect("update alpha matcher");
    git(
        repo.dir.path(),
        ["add", "gateway/run_busy.py", "tests/test_run_busy.py"],
    );
    git(
        repo.dir.path(),
        [
            "commit",
            "-m",
            "Fix Matrix stop colon handling",
            "-m",
            "The Matrix stop command must preserve chat ids.",
        ],
    );

    let output = repo.run([
        "regression",
        "Matrix",
        "stop",
        "colon",
        "--path",
        "gateway/run_busy.py",
        "--symbol",
        "alpha",
        "--good",
        good.as_str(),
    ]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Fix Matrix stop colon handling"));
    assert!(!stdout.contains("Refactor unrelated beta matcher"));
    assert!(stdout.contains("test-history signal"));
    assert!(stdout.contains("tests/test_run_busy.py"));
}

#[test]
fn regression_symbol_span_excludes_following_non_symbol_lines() {
    let repo = TestRepo::new();
    repo.commit(
        "lib.rs",
        b"fn alpha() { return 1; }\nconst VERSION: u8 = 1;\n",
        "Create alpha",
        None,
    );
    let good = repo.head();
    repo.commit(
        "lib.rs",
        b"fn alpha() { return 1; }\nconst VERSION: u8 = 2;\n",
        "Change version only",
        None,
    );

    let output = repo.run([
        "regression",
        "alpha",
        "--path",
        "lib.rs",
        "--symbol",
        "alpha",
        "--good",
        good.as_str(),
    ]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"No supported regression suspects found.\n");
}

#[test]
fn regression_symbol_tracking_survives_overlapping_newest_change() {
    let repo = TestRepo::new();
    repo.commit(
        "lib.rs",
        b"fn alpha(key: &str) -> &str {\n    key\n}\n\nfn beta(key: &str) -> &str {\n    key\n}\n",
        "Create alpha and beta",
        None,
    );
    let good = repo.head();
    repo.commit(
        "lib.rs",
        b"fn alpha(key: &str) -> &str {\n    key.trim()\n}\n\nfn beta(key: &str) -> &str {\n    key\n}\n",
        "Introduce alpha bug",
        Some("The alpha bug changes the returned value."),
    );
    repo.commit(
        "lib.rs",
        b"// shifted\n// shifted\n// shifted\n// shifted\nfn alpha(key: &str) -> &str {\n    key.trim_end()\n}\n\nfn beta(key: &str) -> &str {\n    key\n}\n",
        "Shift file and tweak alpha bug",
        None,
    );

    let output = repo.run([
        "regression",
        "alpha bug",
        "--path",
        "lib.rs",
        "--symbol",
        "alpha",
        "--good",
        good.as_str(),
        "--limit",
        "1",
    ]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Introduce alpha bug"));
    assert!(!stdout.contains("Shift file and tweak alpha"));
}

#[test]
fn regression_symbol_tracking_keeps_unrelated_shifted_changes_out() {
    let repo = TestRepo::new();
    repo.commit(
        "lib.rs",
        b"fn alpha() {\n    return 1;\n}\n\nfn gamma() {\n    return 1;\n}\n",
        "Create alpha and gamma",
        None,
    );
    let good = repo.head();
    repo.commit(
        "lib.rs",
        b"fn alpha() {\n    return 1;\n}\n\nfn gamma() {\n    return 2;\n}\n",
        "Change gamma only",
        Some("The gamma change is unrelated."),
    );
    repo.commit(
        "lib.rs",
        b"// one\n// two\n// three\n// four\n// five\nfn alpha() {\n    return 1;\n}\n\nfn gamma() {\n    return 2;\n}\n",
        "Shift file",
        None,
    );

    let output = repo.run([
        "regression",
        "alpha regression",
        "--path",
        "lib.rs",
        "--symbol",
        "alpha",
        "--good",
        good.as_str(),
    ]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout, "No supported regression suspects found.\n");
}

#[test]
fn regression_uses_out_of_cache_bad_revision_without_indexing_it() {
    let repo = TestRepo::new();
    repo.commit("main.txt", b"main\n", "Main history", None);
    git(repo.dir.path(), ["switch", "-c", "feature"]);
    repo.commit(
        "feature.txt",
        b"feature regression\n",
        "Introduce feature regression",
        Some("The feature path fails after the change."),
    );
    let feature = repo.head();
    git(repo.dir.path(), ["switch", "main"]);

    let output = repo.run([
        "regression",
        "feature",
        "failure",
        "--path",
        "feature.txt",
        "--bad",
        "feature",
    ]);
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Introduce feature regression"));

    let cache = rusqlite::Connection::open(repo.dir.path().join(".gitscry/cache.sqlite"))
        .expect("open cache");
    let indexed: i64 = cache
        .query_row(
            "SELECT COUNT(*) FROM commits WHERE oid = ?1",
            [&feature],
            |row| row.get(0),
        )
        .expect("count indexed feature commits");
    assert_eq!(indexed, 0);
}

#[test]
fn regression_uses_word_boundaries_for_symptom_terms() {
    let repo = TestRepo::new();
    repo.commit("target.txt", b"hidden video\n", "Show hidden video", None);

    let output = repo.run(["regression", "id", "--path", "target.txt"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"No supported regression suspects found.\n");
}

#[test]
fn regression_works_without_a_default_branch_for_explicit_target() {
    let repo = TestRepo::new();
    repo.commit(
        "target.txt",
        b"broken\n",
        "Introduce target regression",
        None,
    );
    let bad = repo.head();
    git(repo.dir.path(), ["branch", "-m", "dev"]);

    let output = repo.run([
        "regression",
        "target",
        "--path",
        "target.txt",
        "--bad",
        bad.as_str(),
    ]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Introduce target regression"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no default branch"));
}

#[test]
fn regression_rejects_invalid_inputs_and_reports_no_result() {
    let repo = TestRepo::new();
    repo.commit("target.txt", b"target\n", "Create target", None);
    let good = repo.head();
    repo.commit("other.txt", b"other\n", "Unrelated change", None);
    let bad = repo.head();

    let no_result = repo.run([
        "regression",
        "anything",
        "--path",
        "target.txt",
        "--good",
        good.as_str(),
        "--bad",
        bad.as_str(),
    ]);
    assert_eq!(no_result.status.code(), Some(0));
    assert_eq!(
        no_result.stdout,
        b"No supported regression suspects found.\n"
    );

    let invalid_revision = repo.run([
        "regression",
        "anything",
        "--path",
        "target.txt",
        "--bad",
        "not-a-revision",
    ]);
    assert_eq!(invalid_revision.status.code(), Some(2));

    let invalid_path = repo.run(["regression", "anything", "--path", "missing.txt"]);
    assert_eq!(invalid_path.status.code(), Some(2));

    let invalid_range = repo.run([
        "regression",
        "anything",
        "--path",
        "target.txt",
        "--good",
        bad.as_str(),
        "--bad",
        good.as_str(),
    ]);
    assert_eq!(invalid_range.status.code(), Some(2));
}

#[test]
fn regression_warns_when_path_history_crosses_a_rename() {
    let repo = TestRepo::new();
    repo.commit("old.txt", b"old\n", "Create old path", None);
    repo.rename("old.txt", "new.txt", "Move regression path");

    let output = repo.run(["regression", "path moved", "--path", "new.txt"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("move/rename boundary"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("move/rename boundary"));
}

#[test]
fn regression_warns_for_shallow_history() {
    let source = TestRepo::new();
    source.commit("target.txt", b"first\n", "First target", None);
    source.commit("target.txt", b"second\n", "Second target", None);
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
        .args(["regression", "target", "--path", "target.txt"])
        .current_dir(&clone)
        .output()
        .expect("run regression on shallow repository");
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stderr).contains("local history is shallow"));
}

#[test]
fn regression_warns_when_cached_history_has_missing_objects() {
    let repo = TestRepo::new();
    repo.commit("target.txt", b"first\n", "First target", None);
    let first = repo.head();
    repo.commit(
        "target.txt",
        b"second\n",
        "Introduce target regression",
        None,
    );
    let blob = git_stdout(repo.dir.path(), ["ls-tree", &first, "target.txt"]);
    let blob = blob.split_whitespace().nth(2).expect("target blob");
    let object_path = repo
        .dir
        .path()
        .join(".git/objects")
        .join(&blob[..2])
        .join(&blob[2..]);
    fs::remove_file(object_path).expect("remove historical blob");

    let indexed = repo.run(["index"]);
    assert_eq!(indexed.status.code(), Some(0));
    let output = repo.run(["regression", "target", "--path", "target.txt"]);
    assert_eq!(output.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("some local objects are missing") || stderr.contains("missing Git objects"),
        "{stderr}"
    );
}

#[test]
fn regression_treats_path_like_symptoms_as_text() {
    let repo = TestRepo::new();
    repo.commit("target.txt", b"timeout\n", "Fix /api/health timeout", None);

    let output = repo.run([
        "regression",
        "timeout",
        "on",
        "/api/health",
        "--path",
        "target.txt",
    ]);
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Fix /api/health timeout"));
}
