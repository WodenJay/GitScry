mod support;

use std::{fs, path::Path, process::Command};

use rusqlite::Connection;
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

#[test]
fn why_symbol_relocations_preserve_historical_material() {
    for whole_file in [true, false] {
        let repo = TestRepo::new();
        repo.commit(
            "src/old.rs",
            b"fn calculate() {\n    let result = 1;\n}\n",
            "Create calculation",
            None,
        );
        let introduction = repo.head();
        repo.commit(
            "src/old.rs",
            b"fn calculate() {\n    let result = 2;\n}\n",
            "Correct calculation",
            Some("Because callers need the corrected result."),
        );
        let modification = repo.head();
        if whole_file {
            repo.rename("src/old.rs", "src/new.rs", "Rename file");
        } else {
            repo.commit(
                "src/new.rs",
                b"fn helper() {}\n",
                "Create destination",
                None,
            );
            fs::write(repo.dir.path().join("src/old.rs"), b"fn remaining() {}\n").unwrap();
            repo.commit(
                "src/new.rs",
                b"fn helper() {}\nfn calculate() {\n    let result = 2;\n}\n",
                "Move calculation",
                None,
            );
            git(repo.dir.path(), ["add", "src/old.rs"]);
            git(repo.dir.path(), ["commit", "--amend", "--no-edit"]);
        }
        let relocation = repo.head();
        repo.index();
        let report = json(
            &repo,
            &[
                "why",
                "src/new.rs",
                "--symbol",
                "calculate",
                "--patch",
                "--json",
            ],
        );
        assert_eq!(
            report["symbol_summary"]["introduction"]["commit_oid"], introduction,
            "{report}"
        );
        assert_eq!(report["matched_count"], 1);
        assert_eq!(
            report["target_related_modifications"][0]["oid"],
            modification
        );
        assert!(
            report["target_related_modifications"][0]
                .to_string()
                .contains("src/old.rs")
        );
        assert!(
            !report["target_related_modifications"]
                .to_string()
                .contains(&relocation)
        );
        assert!(
            report["target_related_modifications"][0]["patch"]
                .to_string()
                .contains("result = 2")
        );
        let scoped = json(
            &repo,
            &[
                "why",
                "src/new.rs",
                "--symbol",
                "calculate",
                "--from-rev",
                modification.as_str(),
                "--patch",
                "--json",
            ],
        );
        assert_eq!(
            scoped["symbol_summary"]["introduction"]["status"],
            "unknown"
        );
        assert!(!scoped.to_string().contains(&introduction));
        assert_eq!(scoped["matched_count"], 0);
        assert!(
            !scoped["target_related_modifications"]
                .to_string()
                .contains(&modification)
        );
        let time_scoped = json(
            &repo,
            &[
                "why",
                "src/new.rs",
                "--symbol",
                "calculate",
                "--since",
                "9999-01-01",
                "--json",
            ],
        );
        assert_eq!(time_scoped["matched_count"], 0);
        assert!(!time_scoped.to_string().contains(&introduction));
        assert!(!time_scoped.to_string().contains(&modification));
        repo.commit(
            "src/new.rs",
            b"fn calculate() { let result = 999; }\n",
            "Later replacement",
            None,
        );
        repo.index();
        let pinned = json(
            &repo,
            &[
                "why",
                "src/new.rs",
                "--symbol",
                "calculate",
                "--at",
                relocation.as_str(),
                "--json",
            ],
        );
        assert_eq!(
            pinned["symbol_summary"]["introduction"]["commit_oid"],
            introduction
        );
        assert_eq!(
            pinned["target_related_modifications"][0]["oid"],
            modification
        );
    }
}

#[test]
fn why_symbol_uncertain_moves_do_not_claim_creation() {
    for ambiguous in [true, false] {
        let repo = TestRepo::new();
        repo.commit(
            "src/old.rs",
            b"fn calculate() { let result = 1; }\n",
            "Create source",
            None,
        );
        if ambiguous {
            repo.commit(
                "src/other.rs",
                b"fn calculate() { let result = 1; }\n",
                "Create second source",
                None,
            );
        }
        repo.commit(
            "src/new.rs",
            b"fn helper() {}\n",
            "Create destination",
            None,
        );
        fs::write(repo.dir.path().join("src/old.rs"), b"fn remaining() {}\n").unwrap();
        if ambiguous {
            fs::write(repo.dir.path().join("src/other.rs"), b"fn another() {}\n").unwrap();
        }
        let contents: &[u8] = if ambiguous {
            b"fn helper() {}\nfn calculate() { let result = 1; }\n"
        } else {
            b"fn helper() {}\nfn calculate() { let result = 999; }\n"
        };
        repo.commit("src/new.rs", contents, "Uncertain move", None);
        git(repo.dir.path(), ["add", "-A"]);
        git(repo.dir.path(), ["commit", "--amend", "--no-edit"]);
        repo.index();
        let report = json(
            &repo,
            &["why", "src/new.rs", "--symbol", "calculate", "--json"],
        );
        assert_eq!(
            report["symbol_summary"]["introduction"]["status"], "unknown",
            "{report}"
        );
        assert_eq!(report["matched_count"], 0);
    }
}

#[test]
fn why_reports_blame_without_inventing_a_line_explanation() {
    let repo = TestRepo::new();
    repo.commit(
        "target.txt",
        b"target\n",
        "Add target",
        Some("Because callers need a stable target, keep this line explicit."),
    );

    repo.index();
    let output = repo.run(["why", "target.txt", "--line", "1", "--limit", "1"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Why at line 1 at revision "));
    assert!(stdout.contains("Attribution: "));
    assert!(stdout.contains("Attribution count: 1"));
    assert!(stdout.contains("Add target"));
    assert!(stdout.contains("Attribution scope: target line."));
    assert!(!stdout.contains("confidence:"));
    assert!(!stdout.contains("Because callers need a stable target"));
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

    repo.index();
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
    assert!(stdout.contains("Why at symbol explain (line 1) at revision "));
    assert!(stdout.contains("Initial symbol"));
    assert!(stdout.contains("Attribution scope: symbol starting line only."));
    assert!(!stdout.contains("Later symbol change"));
    assert!(stdout.contains("Symbol summary:"));
    assert!(stdout.contains("introduction: known"));
    assert!(stdout.contains("anchor-line attribution: known"));
    assert!(stdout.contains("No standalone target-related modifications in the selected scope."));

    let report = json(
        &repo,
        &[
            "why",
            "src/lib.rs",
            "--symbol",
            "explain",
            "--at",
            initial.as_str(),
            "--json",
        ],
    );
    assert_eq!(report["matched_count"], 0);
    assert_eq!(
        report["target_related_modifications"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(report["symbol_summary"]["introduction"]["status"], "known");
    assert_eq!(
        report["symbol_summary"]["anchor_line_attribution"]["status"],
        "known"
    );
}

#[test]
fn why_symbol_separates_introduction_anchor_and_modifications() {
    let repo = TestRepo::new();
    let initial = concat!(
        "fn calculate() {\n",
        "    let result = 1;\n",
        "}\n\n",
        "fn helper() {\n",
        "    let value = 1;\n",
        "}\n",
    );
    repo.commit("src/lib.rs", initial.as_bytes(), "Create calculation", None);
    let introduction = repo.head();

    let body_changed = initial.replace("result = 1", "result = 2");
    repo.commit(
        "src/lib.rs",
        body_changed.as_bytes(),
        "Update calculation behavior",
        Some("Because this fixes the production calculation regression."),
    );
    let body_revision = repo.head();

    let declaration_changed = body_changed.replace("fn calculate()", "fn calculate(input: i32)");
    repo.commit(
        "src/lib.rs",
        declaration_changed.as_bytes(),
        "Change calculation declaration",
        None,
    );
    let declaration = repo.head();

    let neighbor_changed = declaration_changed.replace("value = 1", "value = 2");
    repo.commit(
        "src/lib.rs",
        neighbor_changed.as_bytes(),
        "Tune unrelated helper",
        Some("Because this detailed explanation belongs to another function."),
    );
    repo.index();

    let report = json(
        &repo,
        &[
            "why",
            "src/lib.rs",
            "--symbol",
            "calculate",
            "--limit",
            "1",
            "--json",
        ],
    );
    let summary = &report["symbol_summary"];
    assert_eq!(summary["introduction"]["status"], "known");
    assert_eq!(summary["introduction"]["commit_oid"], introduction);
    assert_eq!(summary["anchor_line_attribution"]["status"], "known");
    assert_eq!(
        summary["anchor_line_attribution"]["commit_oid"],
        declaration
    );
    assert_eq!(report["matched_count"], 2);
    assert_eq!(report["truncated"], true);
    let materials = report["target_related_modifications"].as_array().unwrap();
    assert_eq!(materials.len(), 1);
    assert_eq!(materials[0]["subject"], "Update calculation behavior");

    let complete = json(
        &repo,
        &[
            "why",
            "src/lib.rs",
            "--symbol",
            "calculate",
            "--limit",
            "10",
            "--json",
        ],
    );
    let subjects = complete["target_related_modifications"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|material| material["subject"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(complete["matched_count"], 2);
    assert_eq!(subjects.len(), 2);
    assert!(subjects.contains(&"Update calculation behavior"));
    assert!(subjects.contains(&"Change calculation declaration"));
    assert!(!subjects.contains(&"Tune unrelated helper"));

    let pinned = json(
        &repo,
        &[
            "why",
            "src/lib.rs",
            "--symbol",
            "calculate",
            "--at",
            body_revision.as_str(),
            "--limit",
            "10",
            "--json",
        ],
    );
    assert_eq!(
        pinned["symbol_summary"]["anchor_line_attribution"]["commit_oid"],
        introduction
    );
    assert_eq!(pinned["matched_count"], 1);
    assert!(
        pinned["target_related_modifications"]
            .as_array()
            .unwrap()
            .iter()
            .all(|material| {
                material["subject"] != "Change calculation declaration"
                    && material["subject"] != "Tune unrelated helper"
                    && material["subject"] != "Create calculation"
            })
    );
    let time_scoped = json(
        &repo,
        &[
            "why",
            "src/lib.rs",
            "--symbol",
            "calculate",
            "--at",
            declaration.as_str(),
            "--since",
            "9999-01-01",
            "--json",
        ],
    );
    assert_eq!(time_scoped["matched_count"], 0);
    assert_eq!(
        time_scoped["target_related_modifications"],
        serde_json::json!([])
    );
    assert_eq!(
        time_scoped["symbol_summary"]["introduction"]["status"],
        "unknown"
    );
    assert_eq!(
        time_scoped["symbol_summary"]["anchor_line_attribution"]["status"],
        "unknown"
    );
    assert!(
        time_scoped["symbol_summary"]["introduction"]["reason"]
            .as_str()
            .unwrap()
            .contains("query scope")
    );
    assert_eq!(
        time_scoped["why"]["attribution"]["state"],
        "outside_historical_scope"
    );
    assert!(
        time_scoped["symbol_summary"]["introduction"]
            .get("commit_oid")
            .is_none()
    );
    assert!(
        time_scoped["symbol_summary"]["anchor_line_attribution"]
            .get("commit_oid")
            .is_none()
    );
}
#[test]
fn why_symbol_recreation_starts_a_new_incarnation() {
    let repo = TestRepo::new();
    repo.commit(
        "src/lib.rs",
        b"fn calculate() { let result = 1; }\n",
        "Old creation",
        None,
    );
    let old = repo.head();
    repo.commit(
        "src/lib.rs",
        b"fn calculate() { let result = 2; }\n",
        "Old modification",
        None,
    );
    repo.commit(
        "src/lib.rs",
        b"fn helper() {}\n",
        "Delete calculation",
        None,
    );
    repo.commit(
        "src/lib.rs",
        b"fn helper() {}\nfn calculate() { let result = 3; }\n",
        "Recreate calculation",
        None,
    );
    let recreation = repo.head();
    repo.index();
    let report = json(
        &repo,
        &["why", "src/lib.rs", "--symbol", "calculate", "--json"],
    );
    assert_eq!(
        report["symbol_summary"]["introduction"]["commit_oid"],
        recreation
    );
    assert_eq!(report["matched_count"], 0);
    assert!(!report.to_string().contains(&old));
}

#[test]
fn why_symbol_name_change_preserves_earlier_material() {
    let repo = TestRepo::new();
    repo.commit(
        "src/lib.rs",
        b"fn original() {\n    let result = 1;\n}\n",
        "Create original",
        None,
    );
    let introduction = repo.head();
    repo.commit(
        "src/lib.rs",
        b"fn original() {\n    let result = 2;\n}\n",
        "Change original body",
        Some("Because callers need the corrected result."),
    );
    let modification = repo.head();
    repo.commit(
        "src/lib.rs",
        b"fn calculate() {\n    let result = 2;\n}\n",
        "Rename calculation",
        None,
    );
    let rename = repo.head();
    repo.index();
    let report = json(
        &repo,
        &[
            "why",
            "src/lib.rs",
            "--symbol",
            "calculate",
            "--patch",
            "--json",
        ],
    );
    assert_eq!(
        report["symbol_summary"]["introduction"]["commit_oid"],
        introduction
    );
    assert_eq!(
        report["symbol_summary"]["anchor_line_attribution"]["commit_oid"],
        rename
    );
    assert_eq!(report["matched_count"], 1);
    assert_eq!(
        report["target_related_modifications"][0]["oid"],
        modification
    );
    assert!(
        report["target_related_modifications"][0]["patch"]
            .to_string()
            .contains("result = 2")
    );
}

#[test]
fn why_symbol_introduction_ignores_an_unrelated_deletion() {
    let repo = TestRepo::new();
    repo.commit(
        "src/obsolete.rs",
        b"fn obsolete_helper() {}\n",
        "Create helper",
        None,
    );
    fs::write(
        repo.dir.path().join("src/lib.rs"),
        b"fn calculate() { let result = 1; }\n",
    )
    .expect("create calculation file");
    fs::remove_file(repo.dir.path().join("src/obsolete.rs")).expect("remove helper file");
    git(repo.dir.path(), ["add", "-A"]);
    git(
        repo.dir.path(),
        ["commit", "-m", "Create calculation and remove helper"],
    );
    let introduction = repo.head();
    repo.commit(
        "src/lib.rs",
        b"fn calculate() { let result = 2; }\n",
        "Update calculation body",
        None,
    );
    repo.index();

    let report = json(
        &repo,
        &["why", "src/lib.rs", "--symbol", "calculate", "--json"],
    );
    assert_eq!(report["symbol_summary"]["introduction"]["status"], "known");
    assert_eq!(
        report["symbol_summary"]["introduction"]["commit_oid"],
        introduction
    );
}

#[test]
fn why_symbol_keeps_oldest_range_change_when_introduction_is_unknown() {
    let repo = TestRepo::new();
    repo.commit(
        "src/lib.rs",
        b"fn calculate() { let result = 1; }\n",
        "Create calculation",
        None,
    );
    repo.commit(
        "src/lib.rs",
        b"fn calculate() { let result = 2; }\n",
        "Update calculation body",
        None,
    );
    let body_change = repo.head();
    repo.index();
    fs::write(
        repo.dir.path().join(".git/shallow"),
        format!("{body_change}\n"),
    )
    .expect("mark body change as shallow boundary");

    let report = json(
        &repo,
        &[
            "why",
            "src/lib.rs",
            "--symbol",
            "calculate",
            "--at",
            body_change.as_str(),
            "--json",
        ],
    );
    assert_eq!(
        report["symbol_summary"]["introduction"]["status"],
        "unknown"
    );
    assert_eq!(report["matched_count"], 1);
    assert_eq!(
        report["target_related_modifications"][0]["subject"],
        "Update calculation body"
    );
}

#[test]
fn why_symbol_reports_unknown_introduction_at_a_real_shallow_boundary() {
    let source = TestRepo::new();
    source.commit(
        "src/lib.rs",
        b"fn calculate() { let result = 1; }\n",
        "Create calculation",
        None,
    );
    source.commit(
        "src/lib.rs",
        b"fn calculate() { let result = 2; }\n",
        "Update calculation body",
        None,
    );
    source.commit(
        "src/lib.rs",
        b"fn calculate() { let result = 3; }\n",
        "Update calculation again",
        None,
    );

    let parent = tempfile::tempdir().expect("create clone parent");
    let clone = parent.path().join("clone");
    let cloned = git_command(parent.path())
        .args(["clone", "--depth", "2", "--no-local"])
        .arg(source.dir.path())
        .arg(&clone)
        .output()
        .expect("clone shallow repository");
    assert!(cloned.status.success());
    let boundaries = git_stdout(&clone, ["rev-parse", "--git-path", "shallow"]);
    assert!(
        fs::read_to_string(clone.join(boundaries))
            .expect("read shallow boundary")
            .lines()
            .count()
            > 0
    );

    let indexed = TestRepo::run_at(&clone, ["index"]);
    assert!(
        indexed.status.success(),
        "{}",
        String::from_utf8_lossy(&indexed.stderr)
    );
    let output = TestRepo::run_at(
        &clone,
        ["why", "src/lib.rs", "--symbol", "calculate", "--json"],
    );
    assert_eq!(output.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["symbol_summary"]["introduction"]["status"],
        "unknown"
    );
    assert!(
        report["symbol_summary"]["introduction"]["reason"]
            .as_str()
            .unwrap()
            .contains("shallow")
    );
    assert!(
        report["target_related_modifications"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| change["subject"] == "Update calculation again")
    );
}

#[test]
fn why_symbol_reports_missing_objects_without_unrelated_fallback() {
    let repo = TestRepo::new();
    repo.commit(
        "src/lib.rs",
        b"fn calculate() { let result = 1; }\nfn helper() { let result = 1; }\n",
        "Create symbols",
        None,
    );
    let introduction = repo.head();
    repo.commit(
        "src/lib.rs",
        b"fn calculate() { let result = 2; }\nfn helper() { let result = 1; }\n",
        "Update calculation body",
        None,
    );
    repo.commit(
        "src/lib.rs",
        b"fn calculate() { let result = 2; }\nfn helper() { let result = 2; }\n",
        "Unrelated helper change",
        Some("This change is explanatory because it avoids a stale result."),
    );
    repo.index();

    let spec = format!("{introduction}:src/lib.rs");
    let blob = git_stdout(repo.dir.path(), ["rev-parse", spec.as_str()]);
    let object = repo
        .dir
        .path()
        .join(".git/objects")
        .join(&blob[..2])
        .join(&blob[2..]);
    fs::remove_file(object).expect("remove required historical blob");

    let output = repo.run(["why", "src/lib.rs", "--symbol", "calculate", "--json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["matched_count"], 0);
    let introduction = &report["symbol_summary"]["introduction"];
    assert_eq!(introduction["status"], "unknown");
    let reason = introduction["reason"].as_str().unwrap();
    assert!(
        ["missing", "unavailable", "incomplete"]
            .iter()
            .any(|word| reason.contains(word)),
        "unexpected uncertainty reason: {reason}"
    );
    assert!(
        report["target_related_modifications"]
            .as_array()
            .unwrap()
            .iter()
            .all(|change| change["subject"] != "Unrelated helper change")
    );
    let text_output = repo.run(["why", "src/lib.rs", "--symbol", "calculate"]);
    assert_eq!(text_output.status.code(), Some(0));
    let text = String::from_utf8_lossy(&text_output.stdout);
    assert!(text.contains("introduction: unknown"));
    assert!(text.contains(reason));
}

#[test]
fn why_symbol_follows_first_parent_through_safe_merges_only() {
    let report = |repo: &TestRepo| {
        repo.index();
        let output = repo.run(["why", "src/lib.rs", "--symbol", "calculate", "--json"]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };

    let supported = TestRepo::new();
    supported.commit(
        "src/lib.rs",
        b"fn calculate() { let result = 1; }\n\nfn helper() {}\n",
        "Create calculation",
        None,
    );
    let introduction = supported.head();
    git(supported.dir.path(), ["switch", "-c", "side"]);
    supported.commit(
        "src/lib.rs",
        b"fn calculate() { let result = 1; }\n\nfn helper() {}\nfn side_helper() {}\n",
        "Add helper on side branch",
        None,
    );
    git(supported.dir.path(), ["switch", "main"]);
    supported.commit(
        "src/lib.rs",
        b"fn calculate() { let result = 2; }\n\nfn helper() {}\n",
        "Update calculation",
        None,
    );
    let merge = git_command(supported.dir.path())
        .args(["merge", "--no-ff", "side", "-m", "Merge side helper"])
        .output()
        .expect("merge side helper");
    assert!(
        merge.status.success(),
        "{}",
        String::from_utf8_lossy(&merge.stdout)
    );

    let supported_report = report(&supported);
    assert_eq!(
        supported_report["symbol_summary"]["introduction"]["status"],
        "known"
    );
    assert_eq!(
        supported_report["symbol_summary"]["introduction"]["commit_oid"],
        introduction
    );
    assert!(
        supported_report["target_related_modifications"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| change["subject"] == "Update calculation")
    );

    let uncertain = TestRepo::new();
    uncertain.commit(
        "src/lib.rs",
        b"fn helper() { let result = 1; }\n\nfn anchor() {}\n",
        "Create helper",
        None,
    );
    git(uncertain.dir.path(), ["switch", "-c", "side"]);
    uncertain.commit(
        "src/lib.rs",
        b"fn helper() { let result = 1; }\n\nfn anchor() {}\nfn calculate() { let result = 2; }\n",
        "Add calculation on side branch",
        None,
    );
    git(uncertain.dir.path(), ["switch", "main"]);
    uncertain.commit(
        "src/lib.rs",
        b"fn helper() { let result = 2; }\n\nfn anchor() {}\n",
        "Update helper on first parent",
        None,
    );
    git(
        uncertain.dir.path(),
        ["merge", "--no-ff", "side", "-m", "Merge side calculation"],
    );

    let uncertain_report = report(&uncertain);
    assert_eq!(
        uncertain_report["symbol_summary"]["introduction"]["status"],
        "unknown"
    );
    assert!(
        uncertain_report["symbol_summary"]["introduction"]["reason"]
            .as_str()
            .unwrap()
            .contains("merge history")
    );
}

#[test]
fn why_symbol_preserves_first_parent_edits_when_merge_lineage_is_unknown() {
    let repo = TestRepo::new();
    repo.commit(
        "src/lib.rs",
        b"fn calculate() {\n    let result = 1;\n    let middle_a = 1;\n    let middle_b = 1;\n    let middle_c = 1;\n    let middle_d = 1;\n    let middle_e = 1;\n    let middle_f = 1;\n    let middle_g = 1;\n    let middle_h = 1;\n    let side = 1;\n}\n",
        "Create calculation",
        None,
    );
    git(repo.dir.path(), ["switch", "-c", "side"]);
    repo.commit(
        "src/lib.rs",
        b"fn calculate() {\n    let result = 1;\n    let middle_a = 1;\n    let middle_b = 1;\n    let middle_c = 1;\n    let middle_d = 1;\n    let middle_e = 1;\n    let middle_f = 1;\n    let middle_g = 1;\n    let middle_h = 1;\n    let side = 3;\n}\n",
        "Update calculation on side branch",
        None,
    );
    git(repo.dir.path(), ["switch", "main"]);
    repo.commit(
        "src/lib.rs",
        b"fn calculate() {\n    let result = 2;\n    let middle_a = 1;\n    let middle_b = 1;\n    let middle_c = 1;\n    let middle_d = 1;\n    let middle_e = 1;\n    let middle_f = 1;\n    let middle_g = 1;\n    let middle_h = 1;\n    let side = 1;\n}\n",
        "Update calculation on first parent",
        None,
    );
    let merge = git_command(repo.dir.path())
        .args([
            "merge",
            "--no-ff",
            "side",
            "-m",
            "Merge calculation changes",
        ])
        .output()
        .expect("merge divergent calculation changes");
    assert!(
        merge.status.success(),
        "{}",
        String::from_utf8_lossy(&merge.stdout)
    );

    repo.index();
    let native_history = git_stdout(
        repo.dir.path(),
        ["log", "--format=%s", "-L", "1,12:src/lib.rs", "HEAD"],
    );
    assert!(
        native_history.contains("Update calculation on first parent"),
        "fixture must have native line-range support for the first-parent edit: {native_history}"
    );
    let output = repo.run(["why", "src/lib.rs", "--symbol", "calculate", "--json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["symbol_summary"]["introduction"]["status"],
        "unknown"
    );
    assert!(
        report["symbol_summary"]["introduction"]["reason"]
            .as_str()
            .unwrap()
            .contains("merge history")
    );
    assert!(
        report["target_related_modifications"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| change["subject"] == "Update calculation on first parent"),
        "a native-supported first-parent edit should survive uncertain merge lineage: {report}"
    );
    assert!(
        report["target_related_modifications"]
            .as_array()
            .unwrap()
            .iter()
            .all(|change| change["subject"] != "Update calculation on side branch")
    );
}

#[test]
fn why_symbol_rejects_ambiguous_or_unlocated_declarations() {
    let repo = TestRepo::new();
    repo.commit(
        "src/lib.rs",
        b"fn duplicate() {}\nfn duplicate() {}\n",
        "Duplicate declarations",
        None,
    );
    repo.index();

    let ambiguous = repo.run(["why", "src/lib.rs", "--symbol", "duplicate"]);
    assert_eq!(ambiguous.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&ambiguous.stderr).contains("ambiguous"));
    assert!(String::from_utf8_lossy(&ambiguous.stderr).contains("1, 2"));

    repo.commit(
        "src/lib.rs",
        b"// duplicate() appears in this prose.\nlet value = duplicate();\n",
        "Mention only",
        None,
    );
    repo.index();
    let unlocated = repo.run(["why", "src/lib.rs", "--symbol", "duplicate"]);
    assert_eq!(unlocated.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&unlocated.stderr).contains("declaration"));
}

#[test]
fn why_rejects_explicit_out_of_cache_targets() {
    let repo = TestRepo::new();
    repo.commit("main.txt", b"main\n", "Main history", None);
    git(repo.dir.path(), ["switch", "-c", "feature"]);
    repo.commit("feature.txt", b"feature\n", "Feature-only history", None);
    let feature_oid = repo.head();
    git(repo.dir.path(), ["switch", "main"]);

    repo.index();
    let output = repo.run(["why", "feature.txt", "--line", "1", "--at", "feature"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("outside the published cache generation")
    );

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
fn why_defaults_to_published_cache_tip_on_feature_branch() {
    let repo = TestRepo::new();
    repo.commit("target.txt", b"main\n", "Main target", None);
    let cache_tip = repo.head();
    repo.index();
    git(repo.dir.path(), ["switch", "-c", "feature"]);
    repo.commit("target.txt", b"feature\n", "Feature target", None);

    let output = repo.run(["why", "target.txt", "--line", "1"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains(&format!("Why at line 1 at revision {cache_tip}"))
    );
}

#[test]
fn why_exposes_rename_history_and_rejects_invalid_anchors() {
    let repo = TestRepo::new();
    repo.commit("old.txt", b"kept\n", "Add old path", None);
    repo.rename("old.txt", "new.txt", "Move old path");

    repo.index();
    let output = repo.run(["why", "new.txt", "--line", "1"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Add old path"));
    assert!(stdout.contains("rename events alone are not target-related modifications"));
    assert!(!stdout.contains("rename boundary"));

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

    repo.index();
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

    repo.index();
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

    repo.index();
    let output = repo.run(["why", "target.txt", "--line", "2", "--limit", "10"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("Rewrite third line"));
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

    repo.index();
    let output = repo.run(["why", "f.txt", "--line", "20", "--limit", "10"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("Adjust unrelated line"));
}

#[test]
fn why_blames_paths_with_literal_metacharacters() {
    let repo = TestRepo::new();
    repo.commit("a1.txt", b"wrong\n", "Wrong file", None);
    repo.commit("a[1].txt", b"right\n", "Bracket file", None);

    repo.index();
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

    repo.index();
    let output = repo.run(["why", "target.txt", "--line", "1"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Change target"));
    assert!(!stdout.contains(repo.dir.path().to_str().expect("UTF-8 test path")));
}

#[test]
fn why_works_without_a_main_or_master_default_branch() {
    let repo = TestRepo::new();
    repo.commit("target.txt", b"target\n", "Create target", None);
    repo.index();
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

    repo.index();
    let output = repo.run(["why", "target.txt", "--line", "1"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stderr).contains("blame ignored revisions"));
}

#[test]
fn why_symbol_anchor_reports_starting_line_attribution() {
    let repo = TestRepo::new();
    repo.commit(
        "src/lib.rs",
        b"// explain() is mentioned here.\nlet value = explain();\npub fn explain() {}\n",
        "Declare symbol",
        None,
    );

    repo.index();
    let output = repo.run(["why", "src/lib.rs", "--symbol", "explain"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Why at symbol explain (line 3)"));
}

#[test]
fn why_does_not_count_cochanged_paths_as_target_modifications() {
    let repo = TestRepo::new();
    repo.commit("target.txt", b"target\n", "Create target", None);
    fs::write(repo.dir.path().join("related.txt"), b"related\n").expect("write related file");
    fs::write(repo.dir.path().join("target.txt"), b"target changed\n").expect("update target file");
    git(repo.dir.path(), ["add", "target.txt", "related.txt"]);
    git(
        repo.dir.path(),
        ["commit", "-m", "Change target and related files"],
    );

    repo.index();
    let output = repo.run(["why", "target.txt", "--line", "1"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Attribution: "));
    assert!(stdout.contains("Other file history: 0"));
    assert!(!stdout.contains("related.txt"));
}

#[test]
fn why_rejects_line_one_for_an_empty_file() {
    let repo = TestRepo::new();
    repo.commit("empty.txt", b"", "Create empty file", None);

    repo.index();
    let output = repo.run(["why", "empty.txt", "--line", "1"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn why_handles_binary_files_without_failing() {
    let repo = TestRepo::new();
    repo.commit("binary.bin", b"\0\xffbinary\0", "Create binary", None);

    repo.index();
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

    repo.index();
    let output = repo.run(["why", "module", "--line", "1"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Attribution unavailable:"));
    assert!(stdout.contains("target path is a submodule"));
    assert!(stdout.contains("File history in scope: 0"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("target path is a submodule"));
}

#[test]
fn why_warns_for_shallow_history() {
    let source = TestRepo::new();
    source.commit("target.txt", b"target\n", "Target history", None);
    let parent = tempfile::tempdir().expect("create clone parent");
    let clone = parent.path().join("clone");
    let cloned = git_command(parent.path())
        .args(["clone", "--depth", "1", "--no-local"])
        .arg(source.dir.path())
        .arg(&clone)
        .output()
        .expect("clone shallow repository");
    assert!(cloned.status.success());

    let indexed = Command::new(env!("CARGO_BIN_EXE_gitscry"))
        .arg("index")
        .current_dir(&clone)
        .output()
        .expect("index shallow repository");
    assert!(indexed.status.success());
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

    repo.index();
    let merge_output = repo.run(["why", "target.txt", "--line", "1"]);
    assert_eq!(merge_output.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&merge_output.stdout)
            .contains("Target-line tracing follows first-parent history")
    );

    repo.commit("target.txt", b"latest\n", "Latest target change", None);
    repo.index();
    let limited = repo.run(["why", "target.txt", "--line", "1", "--limit", "1"]);
    let stdout = String::from_utf8_lossy(&limited.stdout);
    assert!(stdout.contains("Omitted "));
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

    repo.index();
    let output = repo.run(["why", "target.txt", "--line", "1"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stderr).contains("blame ignored revisions"));
}

#[test]
fn why_scope_intersects_target_and_filters_target_modifications() {
    let repo = TestRepo::new();
    repo.commit(
        "target.txt",
        b"target\n",
        "Initial target history",
        Some("Because the target line establishes the behavior."),
    );
    let initial = repo.head();
    repo.commit("target.txt", b"later\n", "Later target history", None);
    let later = repo.head();
    repo.index();

    let scoped = json(
        &repo,
        &[
            "why",
            "target.txt",
            "--line",
            "1",
            "--at",
            initial.as_str(),
            "--to-rev",
            later.as_str(),
            "--patch",
            "--json",
        ],
    );
    assert_eq!(scoped["schema_version"], 5);
    assert_eq!(scoped["scope"]["to_rev"], later);
    assert_eq!(scoped["scope"]["target_rev"], initial);
    assert_eq!(scoped["scope"]["cache_tip"], later);
    assert_eq!(
        scoped["target_related_modifications"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(scoped["why"]["attribution"]["state"], "available");
    assert_eq!(
        scoped["why"]["attribution"]["commit"]["subject"],
        "Initial target history"
    );
    let patch = &scoped["why"]["attribution"]["commit"]["patch"];
    assert_eq!(patch["status"], "available");
    assert!(
        patch["hunks"][0]["text"]
            .as_str()
            .unwrap()
            .contains("+target")
    );

    let excluded = json(
        &repo,
        &[
            "why",
            "target.txt",
            "--line",
            "1",
            "--at",
            initial.as_str(),
            "--from-rev",
            initial.as_str(),
            "--to-rev",
            later.as_str(),
            "--json",
        ],
    );
    assert_eq!(excluded["matched_count"], 0);
    assert_eq!(
        excluded["target_related_modifications"],
        serde_json::json!([])
    );
    assert_eq!(
        excluded["why"]["attribution"]["state"],
        "outside_historical_scope"
    );

    let dated = json(
        &repo,
        &[
            "why",
            "target.txt",
            "--line",
            "1",
            "--at",
            initial.as_str(),
            "--since",
            "9999-01-01",
            "--json",
        ],
    );
    assert_eq!(dated["scope"]["since"], "9999-01-01");
    assert_eq!(dated["matched_count"], 0);
    assert_eq!(dated["why"]["counts"]["file_history"], 0);

    let text = repo.run([
        "why",
        "target.txt",
        "--line",
        "1",
        "--at",
        initial.as_str(),
        "--to-rev",
        later.as_str(),
    ]);
    assert_eq!(text.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&text.stdout).contains("intersected with target revision"));
}

#[test]
fn why_scope_hides_out_of_scope_blame_citations() {
    let repo = TestRepo::new();
    repo.commit("target.txt", b"initial\n", "Initial line", None);
    let initial = repo.head();
    repo.commit("target.txt", b"target\n", "Excluded target blame", None);
    let target = repo.head();
    repo.index();

    let report = json(
        &repo,
        &[
            "why",
            "target.txt",
            "--line",
            "1",
            "--at",
            target.as_str(),
            "--to-rev",
            initial.as_str(),
            "--json",
        ],
    );
    assert_eq!(
        report["why"]["attribution"]["state"],
        "outside_historical_scope"
    );
    assert_eq!(
        report["target_related_modifications"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        report["target_related_modifications"][0]["subject"],
        "Initial line"
    );
    assert!(report["why"]["attribution"]["commit"].is_null());
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
fn why_patch_excerpts_follow_line_and_symbol_anchors() {
    let repo = TestRepo::new();
    let initial = concat!(
        "fn run() {\n",
        "    let target_value = \"old\";\n",
        "    let stable_value = 0;\n",
        "}\n\n",
        "fn helper() {\n",
        "    let filler_0 = 0;\n",
        "    let filler_1 = 1;\n",
        "    let filler_2 = 2;\n",
        "    let filler_3 = 3;\n",
        "    let filler_4 = 4;\n",
        "    let filler_5 = 5;\n",
        "    let filler_6 = 6;\n",
        "    let filler_7 = 7;\n",
        "    let helper_value = 1;\n",
        "    let tail_0 = 0;\n",
        "    let tail_1 = 1;\n",
        "}\n",
    );
    repo.commit("src/engine.rs", initial.as_bytes(), "Create engine", None);
    let changed = initial
        .replace("target_value = \"old\"", "target_value = \"new\"")
        .replace("helper_value = 1", "helper_value = 2");
    repo.commit(
        "src/engine.rs",
        changed.as_bytes(),
        "Change target and helper",
        None,
    );
    fs::write(repo.dir.path().join("README.md"), "unrelated docs\n").expect("write unrelated docs");
    git(repo.dir.path(), ["add", "README.md"]);
    let amended = git_command(repo.dir.path())
        .args(["commit", "--amend", "--no-edit"])
        .output()
        .expect("amend target commit with docs");
    assert!(amended.status.success());
    let latest = changed.replace("helper_value = 2", "helper_value = 3");
    repo.commit(
        "src/engine.rs",
        latest.as_bytes(),
        "Touch unrelated helper",
        None,
    );
    repo.index();

    let plain = json(
        &repo,
        &[
            "why",
            "src/engine.rs",
            "--line",
            "2",
            "--limit",
            "10",
            "--json",
        ],
    );
    let patched = json(
        &repo,
        &[
            "why",
            "src/engine.rs",
            "--line",
            "2",
            "--limit",
            "10",
            "--patch",
            "--json",
        ],
    );
    assert_eq!(plain["schema_version"], 5);
    assert_eq!(patched["schema_version"], 5);
    assert!(plain.get("materials").is_none());
    assert_eq!(patched["matched_count"], plain["matched_count"]);
    let plain_modifications = plain["target_related_modifications"].as_array().unwrap();
    let patched_modifications = patched["target_related_modifications"].as_array().unwrap();
    assert_eq!(plain_modifications.len(), patched_modifications.len());
    assert!(
        plain_modifications
            .iter()
            .all(|modification| modification.get("patch").is_none())
    );
    assert!(plain["why"]["attribution"]["commit"].get("patch").is_none());
    for (plain_modification, patched_modification) in
        plain_modifications.iter().zip(patched_modifications)
    {
        let mut patched_modification = patched_modification.clone();
        patched_modification
            .as_object_mut()
            .unwrap()
            .remove("patch");
        assert_eq!(
            &patched_modification, plain_modification,
            "patch must not change target modifications or ranking"
        );
    }

    let attribution = &patched["why"]["attribution"]["commit"];
    assert_eq!(attribution["subject"], "Change target and helper");
    let patch = &attribution["patch"];
    assert_eq!(patch["status"], "available");
    assert_eq!(patch["commit_oid"], attribution["oid"]);
    let hunks = patch["hunks"].as_array().unwrap();
    assert_eq!(hunks.len(), 1);
    assert_eq!(hunks[0]["new_path"], "src/engine.rs");
    let hunk_text = hunks[0]["text"].as_str().unwrap();
    assert!(hunk_text.contains("target_value = \"new\""));
    assert!(!hunk_text.contains("helper_value"));
    assert!(!hunk_text.contains("README.md"));
    assert!(
        patched_modifications
            .iter()
            .all(|modification| { modification["subject"] != "Touch unrelated helper" })
    );
    assert_eq!(patched["why"]["counts"]["other_file_history"], 1);

    let symbol = json(
        &repo,
        &[
            "why",
            "src/engine.rs",
            "--symbol",
            "run",
            "--limit",
            "10",
            "--patch",
            "--json",
        ],
    );
    let symbol_target = symbol["target_related_modifications"]
        .as_array()
        .unwrap()
        .iter()
        .find(|modification| modification["subject"] == "Change target and helper")
        .expect("symbol-changing modification");
    let symbol_hunks = symbol_target["patch"]["hunks"].as_array().unwrap();
    assert_eq!(symbol_hunks.len(), 1);
    assert!(
        symbol_hunks[0]["text"]
            .as_str()
            .unwrap()
            .contains("target_value = \"new\"")
    );
    assert!(
        !symbol_hunks[0]["text"]
            .as_str()
            .unwrap()
            .contains("helper_value")
    );

    let text = repo.run(["why", "src/engine.rs", "--line", "2", "--patch"]);
    assert_eq!(text.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&text.stdout).contains("patch excerpt: available"));
    let plain_text = repo.run(["why", "src/engine.rs", "--line", "2"]);
    assert!(!String::from_utf8_lossy(&plain_text.stdout).contains("patch excerpt:"));
}

#[test]
fn why_scoped_patch_keeps_eligible_modifications_separate_from_attribution() {
    let repo = TestRepo::new();
    repo.commit("target.txt", b"initial\n", "Create target", None);
    repo.commit("target.txt", b"older\n", "Older target change", None);
    let older = repo.head();
    repo.commit("target.txt", b"newer\n", "Newer target owner", None);
    repo.index();

    let report = json(
        &repo,
        &[
            "why",
            "target.txt",
            "--line",
            "1",
            "--to-rev",
            older.as_str(),
            "--patch",
            "--json",
        ],
    );
    assert_eq!(
        report["why"]["attribution"]["state"],
        "outside_historical_scope"
    );
    assert_eq!(report["matched_count"], 2);
    let modifications = report["target_related_modifications"].as_array().unwrap();
    assert_eq!(modifications.len(), 2);
    let older_change = modifications
        .iter()
        .find(|modification| modification["subject"] == "Older target change")
        .expect("older target change remains eligible");
    assert!(
        older_change["basis"]
            .as_array()
            .unwrap()
            .iter()
            .any(|basis| {
                basis
                    .as_str()
                    .unwrap()
                    .contains("changed lines overlap target line 1")
            })
    );
    assert_eq!(older_change["patch"]["status"], "available");
    assert!(
        older_change["patch"]["hunks"][0]["text"]
            .as_str()
            .unwrap()
            .contains("+older")
    );
    assert!(
        modifications
            .iter()
            .all(|modification| modification["subject"] != "Newer target owner")
    );
}

#[test]
fn why_symbol_patch_keeps_body_hunks_after_signature_edit() {
    let repo = TestRepo::new();
    let initial = "fn calculate() {\n    let result = 1;\n}\n";
    repo.commit(
        "src/calculate.rs",
        initial.as_bytes(),
        "Create calculation",
        None,
    );
    let body_changed = initial.replace("result = 1", "result = 2");
    repo.commit(
        "src/calculate.rs",
        body_changed.as_bytes(),
        "Update calculation result",
        None,
    );
    let signature_changed = body_changed.replace("fn calculate()", "fn calculate(input: i32)");
    repo.commit(
        "src/calculate.rs",
        signature_changed.as_bytes(),
        "Add calculation input",
        None,
    );
    repo.index();

    let patched = json(
        &repo,
        &[
            "why",
            "src/calculate.rs",
            "--symbol",
            "calculate",
            "--patch",
            "--json",
        ],
    );
    let body_material = patched["target_related_modifications"]
        .as_array()
        .unwrap()
        .iter()
        .find(|material| material["subject"] == "Update calculation result")
        .expect("body-changing material");
    assert_eq!(body_material["patch"]["status"], "available");
    let hunks = body_material["patch"]["hunks"].as_array().unwrap();
    assert_eq!(hunks.len(), 1);
    assert!(hunks[0]["text"].as_str().unwrap().contains("result = 2"));
}

#[test]
fn why_symbol_patch_ignores_braces_inside_literals_and_comments() {
    let repo = TestRepo::new();
    let initial = concat!(
        "fn parse<'a>(input: &'a str) {\n",
        "    let text = \"}\";\n",
        "    let raw_text = r#\"}\"#;\n",
        "    // }\n",
        "    let delimiter = '}';\n",
        "    let result = 1;\n",
        "}\n",
    );
    repo.commit("src/parser.rs", initial.as_bytes(), "Create parser", None);
    let changed = initial.replace("result = 1", "result = 2");
    repo.commit(
        "src/parser.rs",
        changed.as_bytes(),
        "Update parser result",
        None,
    );
    repo.index();

    let patched = json(
        &repo,
        &[
            "why",
            "src/parser.rs",
            "--symbol",
            "parse",
            "--patch",
            "--json",
        ],
    );
    let material = patched["target_related_modifications"]
        .as_array()
        .unwrap()
        .iter()
        .find(|material| material["subject"] == "Update parser result")
        .expect("body-changing material");
    assert_eq!(material["patch"]["status"], "available");
    assert!(
        material["patch"]["hunks"][0]["text"]
            .as_str()
            .unwrap()
            .contains("result = 2")
    );
}

#[test]
fn why_symbol_patch_ignores_braces_inside_javascript_strings() {
    let repo = TestRepo::new();
    let initial = concat!(
        "function parse(input) {\n",
        "    const quoted = 'closer } stays string';\n",
        "    const object = { value: 'closer } stays string' };\n",
        "\n",
        "\n",
        "\n",
        "\n",
        "\n",
        "\n",
        "\n",
        "\n",
        "    const template = `template } stays string`;\n",
        "    const result = 1;\n",
        "}\n",
    );
    repo.commit("src/parser.js", initial.as_bytes(), "Create parser", None);
    let changed = initial.replace("result = 1", "result = 2");
    repo.commit(
        "src/parser.js",
        changed.as_bytes(),
        "Update parser result",
        None,
    );
    repo.index();

    let patched = json(
        &repo,
        &[
            "why",
            "src/parser.js",
            "--symbol",
            "parse",
            "--patch",
            "--json",
        ],
    );
    let material = patched["target_related_modifications"]
        .as_array()
        .unwrap()
        .iter()
        .find(|material| material["subject"] == "Update parser result")
        .expect("body-changing material");
    assert_eq!(material["patch"]["status"], "available");
    assert!(
        material["patch"]["hunks"][0]["text"]
            .as_str()
            .unwrap()
            .contains("result = 2")
    );
}

#[test]
fn why_separates_attribution_target_modifications_and_other_history() {
    let repo = TestRepo::new();
    let initial = "fn target() {\n    let value = 1;\n    let neighbor = 0;\n}\n";
    repo.commit("src/lib.rs", initial.as_bytes(), "Create target", None);

    let unrelated = initial.replace("neighbor = 0", "neighbor = 1");
    fs::write(repo.dir.path().join("README.md"), "unrelated\n").expect("write unrelated file");
    git(repo.dir.path(), ["add", "README.md"]);
    repo.commit(
        "src/lib.rs",
        unrelated.as_bytes(),
        "Change neighbor and other file",
        Some("Because this explains a separate concern."),
    );

    let target_change = unrelated.replace("value = 1", "value = 2");
    repo.commit(
        "src/lib.rs",
        target_change.as_bytes(),
        "Update target",
        None,
    );
    let attribution_oid = repo.head();
    repo.index();

    let report = json(&repo, &["why", "src/lib.rs", "--line", "2", "--json"]);
    assert_eq!(report["schema_version"], 5);
    assert!(report.get("materials").is_none());
    assert_eq!(
        report["target_related_modifications"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        report["target_related_modifications"][0]["subject"],
        "Create target"
    );
    assert_eq!(report["why"]["anchor"]["kind"], "line");
    assert_eq!(report["why"]["attribution"]["state"], "available");
    assert_eq!(report["why"]["attribution"]["scope"], "target_line");
    assert_eq!(
        report["why"]["attribution"]["commit"]["oid"],
        attribution_oid
    );
    assert_eq!(
        report["why"]["attribution"]["commit"]["consolidated_target_modification"],
        true
    );
    assert_eq!(report["why"]["counts"]["attribution"], 1);
    assert_eq!(
        report["why"]["counts"]["standalone_target_related_modifications"],
        1
    );
    assert_eq!(report["why"]["counts"]["other_file_history"], 1);
    assert_eq!(report["why"]["counts"]["file_history"], 3);
    assert!(
        serde_json::to_string(&report)
            .unwrap()
            .find("confidence")
            .is_none()
    );
    assert_eq!(report["why"]["timeline_follow_up"]["args"][0], "timeline");
    assert_eq!(report["why"]["timeline_follow_up"]["args"][1], "src/lib.rs");
    assert_eq!(
        report["why"]["timeline_follow_up"]["args"][3],
        attribution_oid
    );

    let text = repo.run(["why", "src/lib.rs", "--line", "2"]);
    assert_eq!(text.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&text.stdout);
    assert!(stdout.contains("Attribution"));
    assert!(stdout.contains("Other file history: 1"));
    assert!(!stdout.contains("confidence:"));
}

#[test]
fn why_symbol_rewritten_renamed_move_is_unknown() {
    let repo = TestRepo::new();
    repo.commit(
        "src/old.rs",
        b"fn original() { let result = 1; }\n",
        "Create source",
        None,
    );
    repo.commit(
        "src/new.rs",
        b"fn helper() {}\n",
        "Create destination",
        None,
    );
    fs::write(repo.dir.path().join("src/old.rs"), b"fn remaining() {}\n").unwrap();
    repo.commit(
        "src/new.rs",
        b"fn helper() {}\nfn calculate() { let result = 999; }\n",
        "Rename rewrite and move",
        None,
    );
    git(repo.dir.path(), ["add", "-A"]);
    git(repo.dir.path(), ["commit", "--amend", "--no-edit"]);
    repo.index();
    let report = json(
        &repo,
        &["why", "src/new.rs", "--symbol", "calculate", "--json"],
    );
    assert_eq!(
        report["symbol_summary"]["introduction"]["status"],
        "unknown"
    );
    assert!(
        report["symbol_summary"]["introduction"]["reason"]
            .as_str()
            .unwrap()
            .contains("rewritten relocation")
    );
    assert_eq!(report["matched_count"], 0);
}

#[test]
fn why_symbol_same_name_does_not_override_competing_move() {
    let repo = TestRepo::new();
    repo.commit(
        "src/lib.rs",
        b"fn calculate() { let result = 1; }\n",
        "Create old incarnation",
        None,
    );
    let old = repo.head();
    repo.commit(
        "src/source.rs",
        b"fn original() { let result = 999; }\n",
        "Create different source",
        None,
    );
    fs::write(
        repo.dir.path().join("src/source.rs"),
        b"fn remaining() {}\n",
    )
    .unwrap();
    repo.commit(
        "src/lib.rs",
        b"fn calculate() { let result = 999; }\n",
        "Replace with relocated source",
        None,
    );
    git(repo.dir.path(), ["add", "-A"]);
    git(repo.dir.path(), ["commit", "--amend", "--no-edit"]);
    repo.index();
    let report = json(
        &repo,
        &["why", "src/lib.rs", "--symbol", "calculate", "--json"],
    );
    assert_eq!(
        report["symbol_summary"]["introduction"]["status"],
        "unknown"
    );
    assert!(!report.to_string().contains(&old));
}

#[test]
fn why_does_not_reuse_target_line_across_merge_branches() {
    let repo = TestRepo::new();
    let initial = concat!(
        "fn run() {\n",
        "    let target = 1;\n",
        "    let neighbor = 0;\n",
        "    let tail = 0;\n",
        "}\n",
    );
    repo.commit("src/main.rs", initial.as_bytes(), "Create target", None);

    git(repo.dir.path(), ["switch", "-c", "side"]);
    let side = initial.replace("neighbor = 0", "neighbor = 1");
    repo.commit(
        "src/main.rs",
        side.as_bytes(),
        "Change side branch neighbor",
        None,
    );

    git(repo.dir.path(), ["switch", "main"]);
    let shifted = initial.replace(
        "    let target = 1;",
        "    let inserted = 0;\n    let target = 1;",
    );
    repo.commit(
        "src/main.rs",
        shifted.as_bytes(),
        "Insert before target",
        None,
    );
    git(
        repo.dir.path(),
        ["merge", "--no-ff", "side", "-m", "Merge side branch"],
    );
    repo.index();

    let report = json(&repo, &["why", "src/main.rs", "--line", "3", "--json"]);
    let modifications = report["target_related_modifications"].as_array().unwrap();
    assert_eq!(
        modifications.len(),
        0,
        "the attribution commit is consolidated; side-branch changes are not target-related: {report:#?}"
    );
    assert_eq!(
        report["why"]["attribution"]["commit"]["subject"],
        "Create target"
    );
    assert!(
        report["why"]["attribution"]["commit"]["consolidated_target_modification"]
            .as_bool()
            .unwrap()
    );
    assert_eq!(report["why"]["counts"]["file_history"], 4);
    assert_eq!(report["why"]["counts"]["other_file_history"], 3);
}
