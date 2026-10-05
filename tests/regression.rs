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
            let amend = git_command(self.dir.path())
                .args(["commit", "--amend", "-m", subject, "-m", body])
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

fn set_head_date(repo: &TestRepo, date: &str) {
    let output = git_command(repo.dir.path())
        .args(["commit", "--amend", "--no-edit", "--date", date])
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .expect("amend commit date");
    assert!(
        output.status.success(),
        "git commit --amend failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn regression_excludes_previous_file_incarnation_without_good() {
    let repo = TestRepo::new();
    repo.commit(
        "src/a.rs",
        b"fn timeout() {}\n",
        "Old timeout introduction",
        None,
    );
    git(repo.dir.path(), ["rm", "src/a.rs"]);
    git(repo.dir.path(), ["commit", "-m", "Delete old timeout file"]);
    repo.commit("src/a.rs", b"fn fresh() {}\n", "Create fresh file", None);
    repo.index();

    let output = repo.run(["regression", "timeout", "--path", "src/a.rs", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["materials"], serde_json::json!([]));
}

#[test]
fn regression_reports_current_incarnation_suspect_after_recreation() {
    let repo = TestRepo::new();
    repo.commit("src/a.rs", b"fn timeout() {}\n", "Old timeout", None);
    git(repo.dir.path(), ["rm", "src/a.rs"]);
    git(repo.dir.path(), ["commit", "-m", "Delete old file"]);
    repo.commit("src/a.rs", b"fn fresh() {}\n", "Create fresh file", None);
    repo.commit(
        "src/a.rs",
        b"fn fresh() { panic!(\"timeout\"); }\n",
        "Current timeout",
        None,
    );
    let suspect = repo.head();
    repo.index();

    let output = repo.run(["regression", "timeout", "--path", "src/a.rs", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let materials = report["materials"].as_array().unwrap();
    assert_eq!(materials.len(), 1);
    assert_eq!(materials[0]["citations"][0]["oid"], suspect);
}

#[test]
fn regression_follows_renames_within_the_selected_bad_incarnation() {
    let repo = TestRepo::new();
    repo.commit("src/old.rs", b"fn timeout() {}\n", "Old timeout", None);
    git(repo.dir.path(), ["rm", "src/old.rs"]);
    git(repo.dir.path(), ["commit", "-m", "Delete old file"]);
    repo.commit("src/old.rs", b"fn fresh() {}\n", "Create fresh file", None);
    let creation = repo.head();
    repo.commit(
        "src/old.rs",
        b"fn fresh() { panic!(\"timeout\"); }\n",
        "Current timeout",
        None,
    );
    let suspect = repo.head();
    repo.rename("src/old.rs", "src/new.rs", "Move current file");
    let bad = repo.head();
    git(repo.dir.path(), ["rm", "src/new.rs"]);
    git(repo.dir.path(), ["commit", "-m", "Delete current file"]);
    repo.commit("src/new.rs", b"fn later() {}\n", "Create later file", None);
    repo.index();

    // Scope excludes the rename, but identity still belongs to --bad, not HEAD.
    for symbol in [None, Some("fresh")] {
        let mut args = vec![
            "regression",
            "timeout",
            "--path",
            "src/new.rs",
            "--bad",
            &bad,
            "--to-rev",
            &suspect,
            "--json",
        ];
        if let Some(symbol) = symbol {
            args.extend(["--symbol", symbol]);
        }
        let output = repo.run(args);
        assert_eq!(output.status.code(), Some(0));
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let materials = report["materials"].as_array().unwrap();
        let mut returned = materials
            .iter()
            .map(|material| material["citations"][0]["oid"].as_str().unwrap())
            .collect::<Vec<_>>();
        let mut expected = if symbol.is_some() {
            vec![creation.as_str(), suspect.as_str()]
        } else {
            vec![suspect.as_str()]
        };
        returned.sort_unstable();
        expected.sort_unstable();
        assert_eq!(returned, expected);
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
    repo.index();

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
    repo.index();

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
    repo.index();

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
    assert!(
        output
            .stdout
            .ends_with(b"No supported regression suspects found.\n")
    );
}

#[test]
fn regression_symbol_ignores_deletion_before_symbol() {
    let repo = TestRepo::new();
    repo.commit(
        "src/lib.rs",
        b"const OLD: i32 = 0;\nfn target() { work(); }\n",
        "Create target",
        None,
    );
    let good = repo.head();
    repo.commit(
        "src/lib.rs",
        b"fn target() { work(); }\n",
        "Remove obsolete constant",
        None,
    );
    let bad = repo.head();
    repo.index();

    let output = repo.run([
        "regression",
        "impossible-symptom",
        "--path",
        "src/lib.rs",
        "--symbol",
        "target",
        "--good",
        good.as_str(),
        "--bad",
        bad.as_str(),
    ]);

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("No supported regression suspects found."),
        "{stdout}"
    );
}

#[test]
fn regression_symbol_reports_deletion_inside_symbol() {
    let repo = TestRepo::new();
    repo.commit(
        "src/lib.rs",
        b"fn target() {\n    keep();\n    obsolete();\n    finish();\n}\n",
        "Create target",
        None,
    );
    let good = repo.head();
    repo.commit(
        "src/lib.rs",
        b"fn target() {\n    keep();\n    finish();\n}\n",
        "Remove obsolete call",
        None,
    );
    let bad = repo.head();
    repo.index();

    let output = repo.run([
        "regression",
        "impossible-symptom",
        "--path",
        "src/lib.rs",
        "--symbol",
        "target",
        "--good",
        good.as_str(),
        "--bad",
        bad.as_str(),
    ]);

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Remove obsolete call"), "{stdout}");
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
    repo.index();

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
    let patched = json(
        &repo,
        &[
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
            "--patch",
            "--json",
        ],
    );
    assert_eq!(patched["materials"][0]["subject"], "Introduce alpha bug");
    let patch = &patched["materials"][0]["patch"];
    assert_eq!(patch["status"], "available");
    assert!(
        patch["hunks"][0]["text"]
            .as_str()
            .unwrap()
            .contains("key.trim()")
    );
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
    repo.index();

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
    assert!(stdout.ends_with("No supported regression suspects found.\n"));
    let patched = json(
        &repo,
        &[
            "regression",
            "alpha regression",
            "--path",
            "lib.rs",
            "--symbol",
            "alpha",
            "--good",
            good.as_str(),
            "--patch",
            "--json",
        ],
    );
    assert_eq!(patched["materials"], serde_json::json!([]));
}

#[test]
fn regression_rejects_out_of_cache_bad_revision() {
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
    repo.index();

    let output = repo.run([
        "regression",
        "feature",
        "failure",
        "--path",
        "feature.txt",
        "--bad",
        "feature",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("outside the published cache generation")
    );

    let cache =
        rusqlite::Connection::open(repo.cache_dir().join("cache.sqlite")).expect("open cache");
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
fn regression_refreshes_current_head_before_target_history() {
    let repo = TestRepo::new();
    repo.commit("target.txt", b"main\n", "Main target", None);
    repo.index();
    git(repo.dir.path(), ["switch", "-c", "feature"]);
    repo.commit("target.txt", b"feature\n", "Feature target", None);
    let head = repo.head();

    let output = repo.run(["regression", "target", "--path", "target.txt"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Main target"));
    assert!(stdout.contains("Feature target"));
    assert!(stdout.contains(&format!("commits reachable from {head}")));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("incomplete history coverage"));
}

#[test]
fn regression_uses_word_boundaries_for_symptom_terms() {
    let repo = TestRepo::new();
    repo.commit("target.txt", b"hidden video\n", "Show hidden video", None);
    repo.index();

    let output = repo.run(["regression", "id", "--path", "target.txt"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(
        output
            .stdout
            .ends_with(b"No supported regression suspects found.\n")
    );
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
    repo.index();
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
    assert!(!String::from_utf8_lossy(&output.stderr).contains("no default branch"));
}

#[test]
fn regression_rejects_invalid_inputs_and_reports_no_result() {
    let repo = TestRepo::new();
    repo.commit("target.txt", b"target\n", "Create target", None);
    let good = repo.head();
    repo.commit("other.txt", b"other\n", "Unrelated change", None);
    let bad = repo.head();
    repo.index();

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

    repo.index();
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
    let cloned = git_command(parent.path())
        .args(["clone", "--depth", "1", "--no-local"])
        .arg(source.dir.path())
        .arg(&clone)
        .output()
        .expect("clone shallow repository");
    assert!(cloned.status.success());

    let indexed = TestRepo::run_at(&clone, ["index"]);
    assert!(
        indexed.status.success(),
        "index failed: {}",
        String::from_utf8_lossy(&indexed.stderr)
    );

    let output = support::isolated_gitscry_command(source.user_data_dir())
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
    repo.index();

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

fn json(repo: &TestRepo, args: &[&str]) -> serde_json::Value {
    let output = repo.run(args);
    assert_eq!(
        output.status.code(),
        Some(0),
        "gitscry failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("parse JSON report")
}

#[test]
fn regression_patch_excerpts_prioritize_symptom_and_symbol_hunks() {
    let repo = TestRepo::new();
    let initial = concat!(
        "fn run() {\n",
        "    let state = \"healthy\";\n",
        "    let run_keep = 0;\n",
        "}\n\n",
        "fn connection() {\n",
        "    let gap_0 = 0;\n",
        "    let gap_1 = 1;\n",
        "    let gap_2 = 2;\n",
        "    let gap_3 = 3;\n",
        "    let gap_4 = 4;\n",
        "    let gap_5 = 5;\n",
        "    let gap_6 = 6;\n",
        "    let gap_7 = 7;\n",
        "    let timeout_ms = 100;\n",
        "}\n\n",
        "\n",
        "\n",
        "\n",
        "\n",
        "\n",
        "\n",
        "\n",
        "\n",
        "fn helper() {\n",
        "    let helper_value = 1;\n",
        "}\n",
    );
    repo.commit("src/engine.rs", initial.as_bytes(), "Create engine", None);
    let good = repo.head();
    let changed = initial
        .replace("state = \"healthy\"", "state = \"regressed\"")
        .replace("timeout_ms = 100", "timeout_ms = 10")
        .replace("helper_value = 1", "helper_value = 2");
    repo.commit(
        "src/engine.rs",
        changed.as_bytes(),
        "Introduce timeout regression",
        None,
    );
    fs::write(repo.dir.path().join("README.md"), "unrelated docs\n").expect("write unrelated docs");
    git(repo.dir.path(), ["add", "README.md"]);
    let amended = git_command(repo.dir.path())
        .args(["commit", "--amend", "--no-edit"])
        .output()
        .expect("amend target commit with docs");
    assert!(amended.status.success());
    repo.index();

    let plain = json(
        &repo,
        &[
            "regression",
            "timeout",
            "--path",
            "src/engine.rs",
            "--symbol",
            "run",
            "--good",
            good.as_str(),
            "--limit",
            "10",
            "--json",
        ],
    );
    let patched = json(
        &repo,
        &[
            "regression",
            "timeout",
            "--path",
            "src/engine.rs",
            "--symbol",
            "run",
            "--good",
            good.as_str(),
            "--limit",
            "10",
            "--patch",
            "--json",
        ],
    );
    assert_eq!(plain["schema_version"], 1);
    assert_eq!(patched["schema_version"], 2);
    assert_eq!(patched["matched_count"], plain["matched_count"]);
    assert_eq!(
        patched["materials"].as_array().unwrap().len(),
        plain["materials"].as_array().unwrap().len()
    );
    for (plain_material, patched_material) in plain["materials"]
        .as_array()
        .unwrap()
        .iter()
        .zip(patched["materials"].as_array().unwrap())
    {
        let mut material = patched_material.clone();
        material.as_object_mut().unwrap().remove("patch");
        assert_eq!(
            &material, plain_material,
            "patch must not change material or ranking"
        );
    }
    let material = &patched["materials"][0];
    assert_eq!(material["subject"], "Introduce timeout regression");
    let patch = &material["patch"];
    assert_eq!(patch["status"], "available");
    assert_eq!(patch["commit_oid"], material["citations"][0]["oid"]);
    let hunks = patch["hunks"].as_array().unwrap();
    assert_eq!(hunks.len(), 2);
    let hunk_texts = hunks
        .iter()
        .map(|hunk| hunk["text"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(
        hunk_texts
            .iter()
            .any(|text| text.contains("state = \"regressed\""))
    );
    assert!(
        hunk_texts
            .iter()
            .any(|text| text.contains("timeout_ms = 10"))
    );
    assert!(hunk_texts.iter().all(|text| !text.contains("helper_value")));
    assert!(hunks.iter().all(|hunk| hunk["new_path"] == "src/engine.rs"));
    assert!(plain["materials"][0].get("patch").is_none());

    let text = repo.run([
        "regression",
        "timeout",
        "--path",
        "src/engine.rs",
        "--symbol",
        "run",
        "--good",
        good.as_str(),
        "--patch",
    ]);
    assert_eq!(text.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&text.stdout).contains("patch excerpt: available"));
    let plain_text = repo.run([
        "regression",
        "timeout",
        "--path",
        "src/engine.rs",
        "--symbol",
        "run",
        "--good",
        good.as_str(),
    ]);
    assert!(!String::from_utf8_lossy(&plain_text.stdout).contains("patch excerpt:"));
}

#[test]
fn regression_patch_reports_no_relevant_hunks() {
    let repo = TestRepo::new();
    let initial = "fn run() {\n    let value = 1;\n}\n";
    repo.commit("src/engine.rs", initial.as_bytes(), "Create engine", None);
    let good = repo.head();
    let changed = initial.replace("value = 1", "value = 2");
    repo.commit(
        "src/engine.rs",
        changed.as_bytes(),
        "Investigate timeout report",
        None,
    );
    repo.index();

    let patched = json(
        &repo,
        &[
            "regression",
            "timeout",
            "--path",
            "src/engine.rs",
            "--good",
            good.as_str(),
            "--patch",
            "--json",
        ],
    );
    let material = patched["materials"]
        .as_array()
        .unwrap()
        .iter()
        .find(|material| material["subject"] == "Investigate timeout report")
        .expect("symptom-matching material");
    assert_eq!(material["patch"]["status"], "no_relevant_hunks");
    assert_eq!(material["patch"]["hunks"], serde_json::json!([]));
}

#[test]
fn regression_scope_intersects_the_pinned_good_bad_window() {
    let repo = TestRepo::new();
    repo.commit("app.py", b"return 'safe'\n", "Initial stable version", None);
    let initial = repo.head();
    repo.commit(
        "app.py",
        b"return 'old timeout'\n",
        "Pre-good timeout change",
        None,
    );
    let good = repo.head();
    repo.commit(
        "app.py",
        b"return 'first timeout'\n",
        "First timeout suspect",
        None,
    );
    let first_suspect = repo.head();
    repo.commit(
        "app.py",
        b"return 'second timeout'\n",
        "Second timeout suspect",
        None,
    );
    let bad = repo.head();
    repo.commit(
        "app.py",
        b"return 'after bad timeout'\n",
        "After-bad timeout change",
        None,
    );
    let after_bad = repo.head();
    repo.index();

    let output = repo.run([
        "regression",
        "timeout",
        "--path",
        "app.py",
        "--good",
        good.as_str(),
        "--bad",
        bad.as_str(),
        "--from-rev",
        initial.as_str(),
        "--to-rev",
        after_bad.as_str(),
        "--since",
        "2000-01-01",
        "--until",
        "2099-12-31",
        "--json",
    ]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let subjects = report["materials"]
        .as_array()
        .unwrap()
        .iter()
        .map(|material| material["subject"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(subjects.contains(&"First timeout suspect"));
    assert!(subjects.contains(&"Second timeout suspect"));
    assert!(!subjects.contains(&"Pre-good timeout change"));
    assert!(!subjects.contains(&"After-bad timeout change"));
    assert_eq!(report["scope"]["from_rev"], initial);
    assert_eq!(report["scope"]["to_rev"], after_bad);
    assert_eq!(report["scope"]["since"], "2000-01-01");
    assert_eq!(report["scope"]["until"], "2099-12-31");
    assert_eq!(report["scope"]["cache_tip"], after_bad);

    let narrowed = json(
        &repo,
        &[
            "regression",
            "timeout",
            "--path",
            "app.py",
            "--good",
            good.as_str(),
            "--bad",
            bad.as_str(),
            "--to-rev",
            first_suspect.as_str(),
            "--json",
        ],
    );
    let subjects = narrowed["materials"]
        .as_array()
        .unwrap()
        .iter()
        .map(|material| material["subject"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(subjects.contains(&"First timeout suspect"));
    assert!(!subjects.contains(&"Second timeout suspect"));
    assert_eq!(narrowed["scope"]["to_rev"], first_suspect);
}

#[test]
fn regression_symbol_scope_keeps_coordinate_history_outside_material_scope() {
    let repo = TestRepo::new();
    repo.commit(
        "lib.rs",
        b"fn calculate() {\n    1\n}\n",
        "Create calculation",
        None,
    );
    set_head_date(&repo, "2024-01-01T00:00:00Z");
    let good = repo.head();
    repo.commit(
        "lib.rs",
        b"fn calculate() {\n    2\n}\n",
        "Adjust calculation",
        None,
    );
    set_head_date(&repo, "2024-01-02T00:00:00Z");
    let suspect = repo.head();
    repo.commit(
        "lib.rs",
        b"// one\n// two\n// three\n// four\nfn calculate() {\n    2\n}\n",
        "Shift calculation coordinates",
        None,
    );
    set_head_date(&repo, "2024-01-03T00:00:00Z");
    let bad = repo.head();
    repo.index();

    for scope in [["--until", "2024-01-02"], ["--to-rev", suspect.as_str()]] {
        let report = json(
            &repo,
            &[
                "regression",
                "calculation",
                "--path",
                "lib.rs",
                "--symbol",
                "calculate",
                "--good",
                &good,
                "--bad",
                &bad,
                scope[0],
                scope[1],
                "--patch",
                "--json",
            ],
        );
        assert_eq!(report["matched_count"], 1, "scope: {scope:?}");
        let material = &report["materials"][0];
        assert_eq!(material["citations"][0]["oid"], suspect);
        assert_eq!(material["patch"]["status"], "available");
        assert!(
            material["patch"]["hunks"][0]["text"]
                .as_str()
                .unwrap()
                .contains("+    2")
        );
    }
}
#[test]
fn regression_scope_uses_inclusive_utc_calendar_days() {
    let repo = TestRepo::new();
    repo.commit("app.py", b"return 'safe'\n", "Initial stable version", None);
    set_head_date(&repo, "2024-01-01T00:00:00Z");
    let good = repo.head();
    repo.commit(
        "app.py",
        b"return 'timeout near midnight'\n",
        "First timeout suspect",
        None,
    );
    set_head_date(&repo, "2024-02-01T23:59:59Z");
    repo.commit(
        "app.py",
        b"return 'timeout after midnight'\n",
        "Second timeout suspect",
        None,
    );
    set_head_date(&repo, "2024-02-02T00:00:00Z");
    let bad = repo.head();
    repo.index();

    let report = json(
        &repo,
        &[
            "regression",
            "timeout",
            "--path",
            "app.py",
            "--good",
            good.as_str(),
            "--bad",
            bad.as_str(),
            "--since",
            "2024-02-01",
            "--until",
            "2024-02-01",
            "--json",
        ],
    );
    let subjects = report["materials"]
        .as_array()
        .unwrap()
        .iter()
        .map(|material| material["subject"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(subjects.contains(&"First timeout suspect"));
    assert!(!subjects.contains(&"Second timeout suspect"));
    assert_eq!(report["scope"]["since"], "2024-02-01");
    assert_eq!(report["scope"]["until"], "2024-02-01");

    let invalid = repo.run([
        "regression",
        "timeout",
        "--path",
        "app.py",
        "--good",
        good.as_str(),
        "--bad",
        bad.as_str(),
        "--until",
        "2024-02-30",
    ]);
    assert_eq!(invalid.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("invalid --until value"));
}
