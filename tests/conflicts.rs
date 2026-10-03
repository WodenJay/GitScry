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
                "pub fn service_identity(name: &str) -> String {\n    const LABEL: &str = \"secret\x1btag\";\n    normalize_identity(name, Owner::Service)\n}\n",
            ),
            (
                "src/identity.rs",
                "pub fn identity_key(name: &str) -> String {\n    normalize_identity(name, Owner::Identity)\n}\n",
            ),
            (
                "src/caller.rs",
                "pub fn caller(name: &str) -> String {\n    let label = \"secret\x1btag\";\n    let value = normalize_identity(name, Owner::Service);\n    log_identity(normalize_identity(name, Owner::Service));\n    audit_identity(normalize_identity(name, Owner::Service));\n    publish_identity(normalize_identity(name, Owner::Service));\n    export_identity(normalize_identity(name, Owner::Service));\n    value\n}\n",
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

fn shared_history_fixture() -> (TestRepo, String, String, String, String, String) {
    let repo = TestRepo::new();
    commit(&repo, "a.txt", "route=old\n", "base");
    commit(&repo, "b.txt", "base b\n", "base b");
    let fix = commit(
        &repo,
        "a.txt",
        "route=fast\n",
        "fix: preserve the fast route",
    );
    git(repo.dir.path(), ["revert", "--no-edit", fix.as_str()]);
    let revert = repo.head();
    for version in 1..=4 {
        commit(
            &repo,
            "a.txt",
            &format!("route=intermediate-{version}\n"),
            &format!("routine path edit {version}"),
        );
    }
    let unrelated = commit(
        &repo,
        "unrelated.txt",
        "unrelated\n",
        "fix: unrelated historical behavior",
    );
    git(repo.dir.path(), ["branch", "other"]);
    let ours = commit(&repo, "a.txt", "route=ours\n", "change ours");
    repo.index();
    git(repo.dir.path(), ["checkout", "other"]);
    let theirs = commit(&repo, "a.txt", "route=theirs\n", "change theirs");
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "other");
    (repo, fix, revert, ours, theirs, unrelated)
}

#[test]
fn reports_reverted_shared_history_once_with_source_and_conflict_associations() {
    let (repo, fix, revert, ours, theirs, unrelated) = shared_history_fixture();
    let before = state(&repo);
    let report = json(repo.run(["conflicts", "--json"]));
    let file = &report["files"][0];
    let related = file["related_history"].as_array().unwrap();
    assert_eq!(related[0]["commit"], fix);
    assert_eq!(
        related.iter().filter(|lead| lead["commit"] == fix).count(),
        1
    );
    assert!(!related.iter().any(|lead| lead["commit"] == revert));
    let fix_lead = related.iter().find(|lead| lead["commit"] == fix).unwrap();
    assert_eq!(fix_lead["path"], "a.txt");
    assert_eq!(
        fix_lead["association"],
        "shared pre-merge-base path history"
    );
    assert_eq!(
        fix_lead["related_sides"],
        serde_json::json!(["ours", "theirs"])
    );
    assert_eq!(fix_lead["reverted_by"]["commit"], revert);
    assert_eq!(fix_lead["recorded_reason"], Value::Null);
    assert!(
        fix_lead["reason_source"]
            .as_str()
            .unwrap()
            .contains("revert")
    );
    assert!(
        fix_lead["selection_basis"]
            .as_array()
            .is_some_and(|basis| !basis.is_empty())
    );
    assert!(!fix_lead["regions"].as_array().unwrap().is_empty());
    assert_eq!(file["related_history_total"], 6);
    assert_eq!(file["related_history_truncated"], true);
    assert_eq!(file["sides"][0]["leads"][0]["commit"], ours);
    assert_eq!(file["sides"][1]["leads"][0]["commit"], theirs);
    assert!(!related.iter().any(|lead| lead["commit"] == unrelated));

    let text = repo.run(["conflicts"]);
    assert!(text.status.success());
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(text.contains(&fix) && text.contains(&revert));
    assert!(text.contains("shared pre-merge-base path history"));
    assert!(text.contains("none recorded"));
    assert_eq!(state(&repo), before);
}

#[test]
fn bounds_shared_history_per_conflicted_file() {
    let repo = TestRepo::new();
    commit(&repo, "a.txt", "value=0\n", "base a");
    commit(&repo, "b.txt", "base b\n", "base b");
    for value in 1..=5 {
        commit(
            &repo,
            "a.txt",
            &format!("value={value}\n"),
            &format!("shared change {value}"),
        );
    }
    git(repo.dir.path(), ["branch", "other"]);
    commit(&repo, "a.txt", "value=ours\n", "change ours");
    repo.index();
    git(repo.dir.path(), ["checkout", "other"]);
    commit(&repo, "a.txt", "value=theirs\n", "change theirs");
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "other");

    let report = json(repo.run(["conflicts", "--json"]));
    let file = &report["files"][0];
    assert_eq!(file["related_history"].as_array().unwrap().len(), 3);
    assert_eq!(file["related_history_total"], 6);
    assert_eq!(file["related_history_truncated"], true);
    assert_eq!(report["limits"]["related_history_per_file"], 3);
}
fn state(repo: &TestRepo) -> Vec<Vec<u8>> {
    state_with_paths(repo, &["a.txt", "b.txt"])
}

fn state_with_paths(repo: &TestRepo, paths: &[&str]) -> Vec<Vec<u8>> {
    let root = repo.dir.path();
    let mut state = Vec::new();
    for path in paths {
        state.push(path.as_bytes().to_vec());
        match fs::read(root.join(path)) {
            Ok(contents) => {
                state.push(vec![1]);
                state.push(contents);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => state.push(vec![0]),
            Err(error) => panic!("reading working-tree path {path}: {error}"),
        }
    }
    for args in [
        vec!["ls-files", "--stage", "-z"],
        vec!["ls-files", "--unmerged", "-z"],
        vec!["show-ref", "--head"],
        vec!["status", "--porcelain=v1", "-z"],
        vec!["rev-parse", "HEAD"],
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
fn exposes_earlier_behavior_and_refactor_across_a_detected_rename() {
    let repo = TestRepo::new();
    commit(
        &repo,
        "lib.rs",
        "pub fn score(input: i32) -> i32 {\n    input\n}\n\nfn adjust(input: i32) -> i32 {\n    input\n}\n",
        "base score",
    );
    git(repo.dir.path(), ["branch", "other"]);
    git(repo.dir.path(), ["checkout", "other"]);
    let earlier = commit(
        &repo,
        "lib.rs",
        "pub fn score(input: i32) -> i32 {\n    input\n}\n\nfn adjust(input: i32) -> i32 {\n    input + 1\n}\n",
        "fix score behavior\n\nPreserve the special score adjustment.",
    );
    git(repo.dir.path(), ["mv", "lib.rs", "score.rs"]);
    let refactor = commit(
        &repo,
        "score.rs",
        "pub fn score(input: i32) -> i32 {\n    adjust(input)\n}\n\nfn adjust(input: i32) -> i32 {\n    input + 1\n}\n",
        "refactor score calculation",
    );
    let unrelated = commit(
        &repo,
        "unrelated.txt",
        "unrelated\n",
        "unrelated distraction",
    );
    git(repo.dir.path(), ["checkout", "main"]);
    git(repo.dir.path(), ["mv", "lib.rs", "score.rs"]);
    commit(
        &repo,
        "score.rs",
        "pub fn score(input: i32) -> i32 {\n    input + 2\n}\n\nfn adjust(input: i32) -> i32 {\n    input\n}\n",
        "change score on ours",
    );
    merge(&repo, "other");

    let report = json(repo.run(["conflicts", "--json"]));
    let file = &report["files"][0];
    assert_eq!(file["path"], "score.rs");
    let sides = file["sides"].as_array().unwrap();
    let theirs = sides.iter().find(|side| side["name"] == "theirs").unwrap();
    let leads = theirs["leads"].as_array().unwrap();
    let commits = leads
        .iter()
        .map(|lead| lead["commit"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(commits.contains(&earlier.as_str()));
    assert!(commits.contains(&refactor.as_str()));
    assert!(!commits.contains(&unrelated.as_str()));

    let earlier_lead = leads.iter().find(|lead| lead["commit"] == earlier).unwrap();
    assert_eq!(earlier_lead["selection_basis"], "detected rename lineage");
    assert_eq!(earlier_lead["conflict_path"], "score.rs");
    assert_eq!(earlier_lead["path_changes"][0]["old_path"], "lib.rs");
    assert_eq!(earlier_lead["regions"][0]["new_path"], "lib.rs");

    let text = String::from_utf8_lossy(&repo.run(["conflicts"]).stdout).to_string();
    assert!(text.contains("Selection basis: detected rename lineage"));
    assert!(text.contains("Historical path: lib.rs"));
}

#[test]
fn does_not_join_a_reintroduced_path_to_an_older_incarnation() {
    let repo = TestRepo::new();
    commit(
        &repo,
        "lib.rs",
        "pub fn score() -> i32 { 0 }\n",
        "base score",
    );
    git(repo.dir.path(), ["branch", "other"]);
    git(repo.dir.path(), ["checkout", "other"]);
    let earlier_incarnation = commit(
        &repo,
        "lib.rs",
        "pub fn score() -> i32 { 1 }\n",
        "fix original score behavior\n\nKeep the original scoring rule.",
    );
    git(repo.dir.path(), ["rm", "lib.rs"]);
    git(
        repo.dir.path(),
        ["commit", "-m", "remove original score file"],
    );
    let reintroduced = commit(
        &repo,
        "lib.rs",
        "pub fn score() -> i32 { 100 }\n",
        "introduce new score file",
    );
    let current = commit(
        &repo,
        "lib.rs",
        "pub fn score() -> i32 { 101 }\n",
        "adjust new score behavior",
    );
    git(repo.dir.path(), ["checkout", "main"]);
    commit(
        &repo,
        "lib.rs",
        "pub fn score() -> i32 { 2 }\n",
        "change score on ours",
    );
    merge(&repo, "other");

    let report = json(repo.run(["conflicts", "--json"]));
    assert_eq!(report["coverage_complete"], true);
    let theirs = report["files"][0]["sides"]
        .as_array()
        .unwrap()
        .iter()
        .find(|side| side["name"] == "theirs")
        .unwrap();
    let commits = theirs["leads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|lead| lead["commit"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(commits.contains(&reintroduced.as_str()));
    assert!(commits.contains(&current.as_str()));
    assert!(!commits.contains(&earlier_incarnation.as_str()));
}

#[test]
fn rename_does_not_claim_same_commit_reintroduction() {
    let repo = TestRepo::new();
    let original = "pub fn score() -> i32 {\n    0\n}\n\n// Original scoring implementation.\n";
    commit(&repo, "lib.rs", original, "base score");
    git(repo.dir.path(), ["mv", "lib.rs", "score.rs"]);
    commit(&repo, "score.rs", original, "rename lib.rs to score.rs");
    git(repo.dir.path(), ["branch", "other"]);
    git(repo.dir.path(), ["checkout", "other"]);
    fs::write(
        repo.dir.path().join("score.rs"),
        "pub fn score() -> i32 {\n    1\n}\n\n// Original scoring implementation.\n",
    )
    .unwrap();
    fs::write(
        repo.dir.path().join("lib.rs"),
        "pub fn unrelated() -> i32 { 100 }\n",
    )
    .unwrap();
    git(repo.dir.path(), ["add", "--all"]);
    git(
        repo.dir.path(),
        ["commit", "-m", "change score and reintroduce lib.rs"],
    );
    let reintroduced = repo.head();

    git(repo.dir.path(), ["checkout", "main"]);
    commit(
        &repo,
        "score.rs",
        "pub fn score() -> i32 {\n    2\n}\n\n// Original scoring implementation.\n",
        "change score on ours",
    );
    merge(&repo, "other");

    let report = json(repo.run(["conflicts", "--json"]));
    assert_eq!(report["coverage_complete"], true);
    let file = report["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == "score.rs")
        .unwrap();
    let theirs = file["sides"]
        .as_array()
        .unwrap()
        .iter()
        .find(|side| side["name"] == "theirs")
        .unwrap();
    let lead = theirs["leads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|lead| lead["commit"] == reintroduced)
        .unwrap();
    let path_changes = lead["path_changes"].as_array().unwrap();
    assert_eq!(path_changes.len(), 1);
    assert_eq!(path_changes[0]["status"], "M");
    assert_eq!(path_changes[0]["old_path"], "score.rs");
    assert_eq!(path_changes[0]["new_path"], "score.rs");
}

#[test]
fn reports_same_commit_callers_tests_and_bounded_matching_hunks() {
    let repo = associated_fixture();
    let report = json(repo.run(["conflicts", "--json"]));
    let materials = report["associated_materials"].as_array().unwrap();
    assert_eq!(materials.len(), 2);
    assert_eq!(report["schema_version"], 4);
    assert_eq!(report["associated_materials_truncated"], false);

    let caller = materials
        .iter()
        .find(|material| material["path"] == "src/caller.rs")
        .unwrap();
    assert_eq!(caller["kind"], "changed_code");
    let caller_identities = caller["shared_identities"].as_array().unwrap();
    assert!(caller_identities.contains(&serde_json::json!("normalize_identity")));
    assert!(
        caller_identities
            .iter()
            .any(|identity| identity.as_str().unwrap().contains('\x1b'))
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
            let lead = side["leads"]
                .as_array()
                .unwrap()
                .iter()
                .find(|lead| lead["commit"] == association["lead_commit"])
                .unwrap();
            assert!(
                lead["associated_material_ids"]
                    .as_array()
                    .unwrap()
                    .contains(&material["id"])
            );
            assert_eq!(lead["associated_materials_truncated"], false);
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
    assert!(!text.contains('\x1b'));
    assert!(text.contains("\\u{1b}"));
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
    let deletion = repo.head();
    commit(&repo, "binary", "ours\0\n", "binary ours");
    git(repo.dir.path(), ["checkout", "other"]);
    let modification = commit(&repo, "a.txt", "theirs\n", "edit");
    commit(&repo, "binary", "theirs\0\n", "binary theirs");
    git(repo.dir.path(), ["checkout", "main"]);
    merge(&repo, "other");
    let report = json(repo.run(["conflicts", "--json"]));
    assert_eq!(report["coverage_complete"], false);
    let file = &report["files"][0];
    assert_eq!(file["file_level"], true);
    assert_eq!(file["index_stages"], serde_json::json!([1, 3]));
    assert!(file["unsupported"].is_null());
    let sides = file["sides"].as_array().unwrap();
    assert_eq!(sides.len(), 2);
    assert_eq!(sides[0]["name"], "ours");
    assert_eq!(sides[0]["available_paths"], serde_json::json!([]));
    assert_eq!(sides[1]["name"], "theirs");
    assert_eq!(sides[1]["available_paths"], serde_json::json!(["a.txt"]));
    assert!(
        sides[0]["leads"]
            .as_array()
            .unwrap()
            .iter()
            .any(|lead| lead["commit"] == deletion)
    );
    assert!(
        sides[1]["leads"]
            .as_array()
            .unwrap()
            .iter()
            .any(|lead| lead["commit"] == modification)
    );
    assert!(
        report["files"][1]["unsupported"]
            .as_str()
            .unwrap()
            .contains("binary")
    );
    assert!(report["files"][1]["sides"].as_array().unwrap().is_empty());
}

#[test]
fn traces_rename_delete_conflicts_alongside_text_conflicts_without_git_changes() {
    let repo = TestRepo::new();
    commit(
        &repo,
        "src/old.rs",
        "pub fn score(value: i32) -> i32 {\n    value\n}\n",
        "base score implementation",
    );
    commit(&repo, "notes.txt", "value=base\n", "base notes");
    git(repo.dir.path(), ["branch", "other"]);

    git(repo.dir.path(), ["checkout", "other"]);
    let historical = commit(
        &repo,
        "src/old.rs",
        "pub fn score(value: i32) -> i32 {\n    value + 1\n}\n",
        "increment score",
    );
    git(repo.dir.path(), ["mv", "src/old.rs", "src/new.rs"]);
    git(repo.dir.path(), ["commit", "-m", "rename score file"]);
    commit(&repo, "notes.txt", "value=theirs\n", "edit notes on theirs");

    git(repo.dir.path(), ["checkout", "main"]);
    git(repo.dir.path(), ["rm", "--", "src/old.rs"]);
    git(repo.dir.path(), ["commit", "-m", "delete score file"]);
    let deletion = repo.head();
    commit(&repo, "notes.txt", "value=ours\n", "edit notes on ours");
    merge(&repo, "other");

    let paths = ["src/old.rs", "src/new.rs", "notes.txt"];
    let before = state_with_paths(&repo, &paths);
    let report = json(repo.run(["conflicts", "--json"]));
    assert_eq!(report["coverage_complete"], true);
    assert_eq!(report["files"].as_array().unwrap().len(), 2);

    let files = report["files"].as_array().unwrap();
    let renamed = files
        .iter()
        .find(|file| file["path"] == "src/new.rs")
        .unwrap();
    assert_eq!(renamed["file_level"], true);
    assert_eq!(renamed["index_stages"], serde_json::json!([1, 3]));
    assert!(renamed["unsupported"].is_null());
    let sides = renamed["sides"].as_array().unwrap();
    assert_eq!(sides.len(), 2);
    assert_eq!(sides[0]["name"], "ours");
    assert_eq!(sides[0]["index_stage_present"], false);
    assert_eq!(sides[0]["available_paths"], serde_json::json!([]));
    assert_eq!(sides[1]["name"], "theirs");
    assert_eq!(sides[1]["index_stage_present"], true);
    assert_eq!(
        sides[1]["available_paths"],
        serde_json::json!(["src/new.rs"])
    );
    let deletion_lead = sides[0]["leads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|lead| lead["commit"] == deletion)
        .unwrap();
    assert_eq!(deletion_lead["selection_basis"], "detected rename lineage");
    assert_eq!(deletion_lead["path_changes"][0]["old_path"], "src/old.rs");
    assert!(deletion_lead["path_changes"][0]["new_path"].is_null());
    assert!(
        sides[1]["leads"]
            .as_array()
            .unwrap()
            .iter()
            .any(|lead| lead["commit"] == historical)
    );

    let text = files
        .iter()
        .find(|file| file["path"] == "notes.txt")
        .unwrap();
    assert_eq!(text["file_level"], false);
    assert_eq!(text["index_stages"], serde_json::json!([1, 2, 3]));
    assert_eq!(text["sides"].as_array().unwrap().len(), 2);

    let filtered = json(repo.run(["conflicts", "--path", "src/new.rs", "--json"]));
    assert_eq!(filtered["files"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["files"][0]["path"], "src/new.rs");
    let rendered = repo.run(["conflicts", "--path", "src/new.rs"]);
    assert!(rendered.status.success());
    let rendered = String::from_utf8_lossy(&rendered.stdout);
    assert!(rendered.contains("File-level conflict"));
    assert!(rendered.contains("Index stages: 1, 3"));
    assert!(rendered.contains(&deletion));
    assert!(rendered.contains("Historical path: src/old.rs"));
    assert_eq!(state_with_paths(&repo, &paths), before);
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
