mod support;
use serde_json::Value;
use std::{fs, process::Output};
use support::{TestRepo, git, git_command, git_stdout};

fn commit(repo: &TestRepo, path: &str, text: &str, message: &str) -> String {
    let file_path = repo.dir.path().join(path);
    fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    fs::write(file_path, text).unwrap();
    git(repo.dir.path(), ["add", path]);
    git(repo.dir.path(), ["commit", "-m", message]);
    repo.head()
}
fn json(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn merge(repo: &TestRepo, branch: &str) {
    assert!(
        !git_command(repo.dir.path())
            .args(["merge", branch])
            .output()
            .unwrap()
            .status
            .success()
    );
}
fn fixture() -> (TestRepo, String, String, String, String) {
    let repo = TestRepo::new();
    let base = commit(&repo, "a.txt", "base\n", "base");
    commit(&repo, "b.txt", "base\n", "base b");
    git(repo.dir.path(), ["branch", "other"]);
    let ours = commit(
        &repo,
        "a.txt",
        "ours\n",
        "change ours\n\nPreserve the local route.",
    );
    commit(&repo, "b.txt", "ours b\n", "change b");
    let unrelated = commit(&repo, "unrelated.txt", "ours\n", "unrelated distraction");
    repo.index();
    git(repo.dir.path(), ["checkout", "other"]);
    let theirs = commit(&repo, "a.txt", "theirs\n", "change theirs");
    commit(&repo, "b.txt", "theirs b\n", "change other b");
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "other");
    (repo, base, ours, theirs, unrelated)
}

fn commit_files(repo: &TestRepo, files: &[(&str, &str)], message: &str) -> String {
    for (path, text) in files {
        let file_path = repo.dir.path().join(path);
        fs::create_dir_all(file_path.parent().unwrap()).unwrap();
        fs::write(file_path, text).unwrap();
        git(repo.dir.path(), ["add", path]);
    }
    git(repo.dir.path(), ["commit", "-m", message]);
    repo.head()
}

fn associated_fixture() -> TestRepo {
    let repo = TestRepo::new();
    commit_files(
        &repo,
        &[
            (
                "src/service.rs",
                "pub fn service_identity(name: &str) -> String {\n    normalize_identity(name)\n}\n",
            ),
            (
                "src/identity.rs",
                "pub fn identity_key(name: &str) -> String {\n    normalize_identity(name)\n}\n",
            ),
            (
                "src/caller.rs",
                "pub fn caller(name: &str) -> String {\n    normalize_identity(name)\n}\n",
            ),
            (
                "tests/identity_regression.rs",
                "fn identity_regression() {\n    assert_eq!(normalize_identity(\"before\"), \"before\");\n}\n",
            ),
            ("src/unrelated.rs", "fn unrelated_before() {}\n"),
        ],
        "base identity behavior",
    );
    git(repo.dir.path(), ["branch", "other"]);
    commit_files(
        &repo,
        &[
            (
                "src/service.rs",
                "pub fn service_identity(name: &str) -> String {\n    normalize_identity(name, Owner::Service)\n}\n",
            ),
            (
                "src/identity.rs",
                "pub fn identity_key(name: &str) -> String {\n    normalize_identity(name, Owner::Identity)\n}\n",
            ),
            (
                "src/caller.rs",
                "pub fn caller(name: &str) -> String {\n    let value = normalize_identity(name, Owner::Service);\n    log_identity(normalize_identity(name, Owner::Service));\n    audit_identity(normalize_identity(name, Owner::Service));\n    publish_identity(normalize_identity(name, Owner::Service));\n    export_identity(normalize_identity(name, Owner::Service));\n    value\n}\n",
            ),
            ("src/unrelated.rs", "fn unrelated_ours() {}\n"),
        ],
        "assign identity ownership and update callers",
    );
    commit(
        &repo,
        "docs/branch_only.md",
        "unrelated branch-only change\n",
        "unrelated branch commit",
    );
    repo.index();
    git(repo.dir.path(), ["checkout", "other"]);
    commit_files(
        &repo,
        &[
            (
                "src/service.rs",
                "pub fn service_identity(name: &str) -> String {\n    normalize_identity(name, IdentityKey::Canonical)\n}\n",
            ),
            (
                "src/identity.rs",
                "pub fn identity_key(name: &str) -> String {\n    normalize_identity(name, IdentityKey::Canonical)\n}\n",
            ),
            (
                "tests/identity_regression.rs",
                "fn identity_regression() {\n    assert_eq!(normalize_identity(\"ID-42\"), \"id-42\");\n}\n",
            ),
            ("docs/same_commit_noise.md", "unrelated same-commit note\n"),
        ],
        "fix identity handling and add regression coverage",
    );
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "other");
    repo
}
fn state(repo: &TestRepo) -> Vec<Vec<u8>> {
    let root = repo.dir.path();
    let mut state = vec![
        fs::read(root.join("a.txt")).unwrap(),
        fs::read(root.join("b.txt")).unwrap(),
    ];
    for args in [
        vec!["ls-files", "--stage", "-z"],
        vec!["show-ref"],
        vec!["status", "--porcelain=v1", "-z"],
    ] {
        state.push(git_command(root).args(args).output().unwrap().stdout);
    }
    for name in ["MERGE_HEAD", "MERGE_MSG", "ORIG_HEAD", "index"] {
        state.push(fs::read(repo.common_dir().join(name)).unwrap());
    }
    state
}

#[test]
fn retrieves_both_sides_prepares_uncached_branch_and_reuses_history_without_git_changes() {
    let (repo, _base, ours, theirs, unrelated) = fixture();
    let before = state(&repo);
    let report = json(repo.run(["conflicts", "--json"]));
    assert_eq!(report["ours"], repo.head());
    assert_eq!(
        report["theirs"],
        git_stdout(repo.dir.path(), ["rev-parse", "other"])
    );
    assert_eq!(report["files"].as_array().unwrap().len(), 2);
    assert_eq!(report["coverage_complete"], true);
    let sides = report["files"][0]["sides"].as_array().unwrap();
    assert_eq!(sides[0]["name"], "ours");
    assert_eq!(sides[0]["leads"][0]["commit"], ours);
    assert_eq!(sides[1]["name"], "theirs");
    assert_eq!(sides[1]["leads"][0]["commit"], theirs);
    assert!(
        sides[0]["leads"][0]["recorded_reason"]
            .as_str()
            .unwrap()
            .contains("Preserve the local route")
    );
    assert!(sides[1]["leads"][0]["recorded_reason"].is_null());
    assert!(
        !sides[0]["leads"][0]["regions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(sides.iter().all(|side| {
        side["leads"]
            .as_array()
            .unwrap()
            .iter()
            .all(|lead| lead["commit"] != unrelated)
    }));
    let again = json(repo.run(["conflicts", "--json"]));
    assert_eq!(report, again);
    let text = repo.run(["conflicts"]);
    assert!(text.status.success());
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(text.contains(&ours) && text.contains(&theirs));
    assert!(text.contains("none recorded"));
    assert!(!text.contains("<<<<<<<"));
    assert_eq!(state(&repo), before);
    let selected = json(repo.run(["conflicts", "--path", "b.txt", "--json"]));
    assert_eq!(selected["files"].as_array().unwrap().len(), 1);
    assert_eq!(selected["files"][0]["path"], "b.txt");
    assert!(
        !repo
            .run(["conflicts", "--path", "missing"])
            .status
            .success()
    );
    assert!(!repo.run(["context"]).status.success());
}

#[test]
fn reports_same_commit_callers_tests_and_bounded_matching_hunks() {
    let repo = associated_fixture();
    let report = json(repo.run(["conflicts", "--json"]));
    let materials = report["associated_materials"].as_array().unwrap();
    assert_eq!(materials.len(), 2);
    assert_eq!(report["schema_version"], 2);
    assert_eq!(report["associated_materials_truncated"], false);

    let caller = materials
        .iter()
        .find(|material| material["path"] == "src/caller.rs")
        .unwrap();
    assert_eq!(caller["kind"], "changed_code");
    assert_eq!(
        caller["shared_identities"],
        serde_json::json!(["normalize_identity"])
    );
    assert_eq!(caller["associated_with"].as_array().unwrap().len(), 2);
    assert!(
        caller["associated_with"]
            .as_array()
            .unwrap()
            .iter()
            .all(|association| association["side"] == "ours")
    );
    assert_eq!(caller["hunks"][0]["lines"].as_array().unwrap().len(), 4);
    assert_eq!(caller["hunks"][0]["excerpt_truncated"], true);
    assert_eq!(caller["hunks_truncated"], true);

    let test = materials
        .iter()
        .find(|material| material["path"] == "tests/identity_regression.rs")
        .unwrap();
    assert_eq!(test["kind"], "test");
    assert_eq!(
        test["shared_identities"],
        serde_json::json!(["normalize_identity"])
    );
    assert_eq!(test["associated_with"].as_array().unwrap().len(), 2);
    assert!(
        test["associated_with"]
            .as_array()
            .unwrap()
            .iter()
            .all(|association| association["side"] == "theirs")
    );
    assert!(materials.iter().all(|material| {
        !material["path"].as_str().unwrap().contains("unrelated")
            && material["hunks"].as_array().unwrap().len() <= 3
    }));
    for material in materials {
        let associations = material["associated_with"].as_array().unwrap();
        let mut conflict_paths = associations
            .iter()
            .map(|association| association["conflict_path"].as_str().unwrap())
            .collect::<Vec<_>>();
        conflict_paths.sort_unstable();
        assert_eq!(conflict_paths, ["src/identity.rs", "src/service.rs"]);
        for association in associations {
            assert_eq!(association["lead_commit"], material["commit"]);
            let file = report["files"]
                .as_array()
                .unwrap()
                .iter()
                .find(|file| file["path"] == association["conflict_path"])
                .unwrap();
            let side = file["sides"]
                .as_array()
                .unwrap()
                .iter()
                .find(|side| side["name"] == association["side"])
                .unwrap();
            assert!(side["leads"].as_array().unwrap().iter().any(|lead| {
                lead["commit"] == association["lead_commit"]
                    && lead["associated_material_ids"]
                        .as_array()
                        .unwrap()
                        .contains(&material["id"])
            }));
        }
    }

    let selected = json(repo.run(["conflicts", "--path", "src/service.rs", "--json"]));
    assert_eq!(selected["files"].as_array().unwrap().len(), 1);
    assert_eq!(selected["files"][0]["path"], "src/service.rs");
    assert!(
        selected["associated_materials"]
            .as_array()
            .unwrap()
            .iter()
            .all(
                |material| material["associated_with"].as_array().unwrap().len() == 1
                    && material["associated_with"][0]["conflict_path"] == "src/service.rs"
            )
    );

    let text = repo.run(["conflicts"]);
    assert!(text.status.success());
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(text.contains("Associated material"));
    assert!(text.contains("src/caller.rs"));
    assert!(text.contains("tests/identity_regression.rs"));
    assert!(text.contains("normalize_identity"));
    assert!(text.contains("excerpt truncated"));
}

#[test]
fn reports_output_bounds_and_absent_reasons() {
    let (repo, _, _, _, _) = fixture();
    git(repo.dir.path(), ["merge", "--abort"]);
    git(repo.dir.path(), ["checkout", "other"]);
    commit(
        &repo,
        "a.txt",
        "more theirs\n",
        &format!("more\n\n{}", "r".repeat(1100)),
    );
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "other");
    let report = json(repo.run(["conflicts", "--limit", "1", "--json"]));
    let side = &report["files"][0]["sides"][1];
    assert_eq!(side["total_leads"], 2);
    assert_eq!(side["truncated"], true);
    assert_eq!(side["leads"][0]["message_truncated"], true);
    assert_eq!(
        side["leads"][0]["recorded_reason"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        1000
    );
    assert!(!repo.run(["conflicts", "--limit", "0"]).status.success());
}

#[test]
fn rejects_non_merge_operations_and_unrelated_histories() {
    let repo = TestRepo::new();
    commit(&repo, "a.txt", "base\n", "base");
    assert!(!repo.run(["conflicts"]).status.success());
    for name in ["rebase-merge", "rebase-apply"] {
        fs::create_dir(repo.common_dir().join(name)).unwrap();
        let output = repo.run(["conflicts"]);
        assert!(String::from_utf8_lossy(&output.stderr).contains("ordinary merge"));
        fs::remove_dir(repo.common_dir().join(name)).unwrap();
    }
    fs::write(repo.common_dir().join("CHERRY_PICK_HEAD"), repo.head()).unwrap();
    assert!(String::from_utf8_lossy(&repo.run(["conflicts"]).stderr).contains("cherry-pick"));
    fs::remove_file(repo.common_dir().join("CHERRY_PICK_HEAD")).unwrap();
    git(repo.dir.path(), ["checkout", "--orphan", "unrelated"]);
    commit(&repo, "a.txt", "unrelated\n", "root");
    git(repo.dir.path(), ["checkout", "main"]);
    assert!(
        !git_command(repo.dir.path())
            .args(["merge", "--allow-unrelated-histories", "unrelated"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let output = repo.run(["conflicts"]);
    assert!(String::from_utf8_lossy(&output.stderr).contains("one merge base"));
}

#[test]
fn explicitly_reports_file_level_and_binary_conflicts() {
    let repo = TestRepo::new();
    commit(&repo, "a.txt", "base\n", "base");
    commit(&repo, "binary", "base\0\n", "binary");
    git(repo.dir.path(), ["branch", "other"]);
    git(repo.dir.path(), ["rm", "a.txt"]);
    git(repo.dir.path(), ["commit", "-m", "delete"]);
    commit(&repo, "binary", "ours\0\n", "binary ours");
    git(repo.dir.path(), ["checkout", "other"]);
    commit(&repo, "a.txt", "theirs\n", "edit");
    commit(&repo, "binary", "theirs\0\n", "binary theirs");
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "other");
    let report = json(repo.run(["conflicts", "--json"]));
    assert_eq!(report["coverage_complete"], false);
    assert!(
        report["files"][0]["unsupported"]
            .as_str()
            .unwrap()
            .contains("file-level")
    );
    assert!(
        report["files"][1]["unsupported"]
            .as_str()
            .unwrap()
            .contains("binary")
    );
    assert!(report["files"][0]["sides"].as_array().unwrap().is_empty());
}

#[test]
fn rejects_multiple_endpoints_and_reports_cache_preparation_failure() {
    let (repo, _, _, _, _) = fixture();
    let merge_head = repo.common_dir().join("MERGE_HEAD");
    let original = fs::read_to_string(&merge_head).unwrap();
    fs::write(&merge_head, format!("{original}{original}")).unwrap();
    let output = repo.run(["conflicts"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("multiple merge endpoints"));
    fs::write(&merge_head, original).unwrap();
    fs::remove_dir_all(repo.cache_dir()).unwrap();
    fs::write(repo.cache_dir(), "obstruct cache preparation").unwrap();
    let output = repo.run(["conflicts", "--json"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cache"));
}

#[test]
fn shallow_history_returns_two_sides_with_explicit_incomplete_coverage() {
    let (repo, _, ours, theirs, _) = fixture();
    let base = git_stdout(repo.dir.path(), ["merge-base", "main", "other"]);
    fs::write(repo.common_dir().join("shallow"), format!("{base}\n")).unwrap();
    fs::remove_dir_all(repo.cache_dir()).unwrap();
    let report = json(repo.run(["conflicts", "--json"]));
    assert_eq!(report["coverage_complete"], false);
    assert!(!report["warnings"].as_array().unwrap().is_empty());
    assert_eq!(report["files"][0]["sides"][0]["leads"][0]["commit"], ours);
    assert_eq!(report["files"][0]["sides"][1]["leads"][0]["commit"], theirs);
}

#[test]
fn discloses_non_utf8_commit_message_decoding() {
    let (repo, _, _, _, _) = fixture();
    git(repo.dir.path(), ["merge", "--abort"]);
    git(repo.dir.path(), ["checkout", "other"]);
    fs::write(repo.dir.path().join("a.txt"), "latest theirs\n").unwrap();
    git(repo.dir.path(), ["add", "a.txt"]);
    fs::write(repo.dir.path().join("message"), b"latest\n\nReason: \xff\n").unwrap();
    git(
        repo.dir.path(),
        [
            "-c",
            "i18n.commitEncoding=ISO-8859-1",
            "commit",
            "-F",
            "message",
        ],
    );
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "other");
    let report = json(repo.run(["conflicts", "--json"]));
    assert_eq!(
        report["files"][0]["sides"][1]["leads"][0]["message_lossy"],
        true
    );
    assert!(
        report["limitations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str().unwrap().contains("UTF-8"))
    );
}
