mod support;

use std::{fs, path::Path, process::Output};

use support::{TestRepo, git, git_command, git_stdout};

impl TestRepo {
    fn commit_files(&self, files: &[(&str, &[u8])], message: &str) {
        for (path, contents) in files {
            let path = Path::new(path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(self.dir.path().join(parent)).expect("create parent directory");
            }
            fs::write(self.dir.path().join(path), contents).expect("write tracked file");
        }
        git(self.dir.path(), ["add", "--all"]);
        git(self.dir.path(), ["commit", "-m", message]);
    }

    fn commit_files_at(
        &self,
        files: &[(&str, &[u8])],
        message: &str,
        author_date: &str,
        committer_date: &str,
    ) {
        for (path, contents) in files {
            let path = Path::new(path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(self.dir.path().join(parent)).expect("create parent directory");
            }
            fs::write(self.dir.path().join(path), contents).expect("write tracked file");
        }
        git(self.dir.path(), ["add", "--all"]);
        let output = git_command(self.dir.path())
            .args(["commit", "-m", message])
            .env("GIT_AUTHOR_DATE", author_date)
            .env("GIT_COMMITTER_DATE", committer_date)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn remove(&self, path: &str, message: &str) {
        fs::remove_file(self.dir.path().join(path)).expect("remove tracked file");
        git(self.dir.path(), ["add", "--all"]);
        git(self.dir.path(), ["commit", "-m", message]);
    }

    fn rename(&self, old: &str, new: &str, message: &str) {
        if let Some(parent) = Path::new(new).parent() {
            fs::create_dir_all(self.dir.path().join(parent)).expect("create rename directory");
        }
        git(self.dir.path(), ["mv", old, new]);
        git(self.dir.path(), ["commit", "-m", message]);
    }

    fn commit_mass_change(&self, seed: &str, candidate: &str) {
        fs::write(self.dir.path().join(seed), b"seed changed\n").expect("write seed file");
        fs::write(self.dir.path().join(candidate), b"candidate changed\n")
            .expect("write candidate file");
        fs::create_dir_all(self.dir.path().join("generated")).expect("create generated directory");
        for index in 0..51 {
            let path = format!("generated/{index}.txt");
            fs::write(self.dir.path().join(path), b"generated\n").expect("write generated file");
        }
        git(self.dir.path(), ["add", "--all"]);
        git(self.dir.path(), ["commit", "-m", "mass change"]);
    }

    fn commit_unrelated_mass_change(&self) {
        fs::create_dir_all(self.dir.path().join("unrelated")).expect("create unrelated directory");
        for index in 0..51 {
            let path = format!("unrelated/{index}.txt");
            fs::write(self.dir.path().join(path), b"unrelated\n").expect("write unrelated file");
        }
        git(self.dir.path(), ["add", "--all"]);
        git(self.dir.path(), ["commit", "-m", "unrelated mass change"]);
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn related_keeps_case_distinct_paths_as_distinct_history() {
    let repo = TestRepo::new();
    git(repo.dir.path(), ["config", "core.ignorecase", "false"]);
    repo.commit_files(&[("src/template.rs", b"same\n")], "base");
    let blob = git_stdout(repo.dir.path(), ["rev-parse", "HEAD:src/template.rs"]);
    git(
        repo.dir.path(),
        ["update-index", "--force-remove", "src/template.rs"],
    );
    for path in ["src/Foo.rs", "src/foo.rs"] {
        let entry = format!("100644,{blob},{path}");
        git(
            repo.dir.path(),
            ["update-index", "--add", "--cacheinfo", entry.as_str()],
        );
    }
    let tree = git_stdout(repo.dir.path(), ["write-tree"]);
    let parent = repo.head();
    let commit = git_stdout(
        repo.dir.path(),
        [
            "commit-tree",
            tree.as_str(),
            "-p",
            parent.as_str(),
            "-m",
            "add case-distinct paths",
        ],
    );
    git(
        repo.dir.path(),
        ["update-ref", "refs/heads/main", commit.as_str()],
    );
    fs::write(
        repo.dir.path().join("different-content"),
        b"lowercase change\n",
    )
    .expect("write distinct blob content");
    let changed_blob = git_stdout(repo.dir.path(), ["hash-object", "-w", "different-content"]);
    for path in ["src/foo.rs", "src/Beta.rs"] {
        let entry = format!("100644,{changed_blob},{path}");
        git(
            repo.dir.path(),
            ["update-index", "--add", "--cacheinfo", entry.as_str()],
        );
    }
    let tree = git_stdout(repo.dir.path(), ["write-tree"]);
    let follow_up = git_stdout(
        repo.dir.path(),
        [
            "commit-tree",
            tree.as_str(),
            "-p",
            commit.as_str(),
            "-m",
            "change lowercase path with Beta",
        ],
    );
    git(
        repo.dir.path(),
        ["update-ref", "refs/heads/main", follow_up.as_str()],
    );
    repo.index();

    for (seed, candidate) in [("src/Foo.rs", "src/foo.rs"), ("src/foo.rs", "src/Foo.rs")] {
        let output = repo.run(["related", seed]);
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        let rendered = stdout(&output);
        assert!(
            rendered.contains(&format!("candidate path: {candidate}")),
            "{seed}: {rendered}"
        );
        if seed == "src/Foo.rs" {
            assert!(
                !rendered.contains("candidate path: src/Beta.rs"),
                "case-distinct history leaked into exact path results: {rendered}"
            );
        } else {
            assert!(
                rendered.contains("candidate path: src/Beta.rs"),
                "{rendered}"
            );
        }
    }
}

#[test]
fn related_does_not_treat_case_distinct_mass_change_as_exact_seed() {
    let repo = TestRepo::new();
    git(repo.dir.path(), ["config", "core.ignorecase", "false"]);
    repo.commit_files(&[("src/foo.rs", b"lowercase\n")], "lowercase history");
    repo.commit_mass_change("src/foo.rs", "src/Beta.rs");

    let blob = git_stdout(repo.dir.path(), ["rev-parse", "HEAD:src/foo.rs"]);
    for path in ["src/Foo.rs", "src/Alpha.rs"] {
        let entry = format!("100644,{blob},{path}");
        git(
            repo.dir.path(),
            ["update-index", "--add", "--cacheinfo", entry.as_str()],
        );
    }
    let tree = git_stdout(repo.dir.path(), ["write-tree"]);
    let parent = repo.head();
    let commit = git_stdout(
        repo.dir.path(),
        [
            "commit-tree",
            tree.as_str(),
            "-p",
            parent.as_str(),
            "-m",
            "add exact-case relation",
        ],
    );
    git(
        repo.dir.path(),
        ["update-ref", "refs/heads/main", commit.as_str()],
    );
    repo.index();

    let output = repo.run(["related", "src/Foo.rs"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let rendered = stdout(&output);
    assert!(
        rendered.contains("candidate path: src/Alpha.rs"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("candidate path: src/Beta.rs"),
        "case-distinct mass change leaked into exact path results: {rendered}"
    );
    assert!(
        !rendered.contains("mass-change commits excluded"),
        "{rendered}"
    );
}

#[test]
fn related_merges_multiple_seeds_and_cites_one_candidate_once() {
    let repo = TestRepo::new();
    repo.commit_files(&[("src/alpha.rs", b"alpha\n")], "alpha");
    repo.commit_files(&[("src/beta.rs", b"beta\n")], "beta");
    repo.commit_files(
        &[
            ("src/alpha.rs", b"alpha one\n"),
            ("src/shared.rs", b"one\n"),
        ],
        "alpha shared",
    );
    repo.commit_files(
        &[("src/beta.rs", b"beta one\n"), ("src/shared.rs", b"two\n")],
        "beta shared",
    );
    repo.commit_files(
        &[
            ("src/alpha.rs", b"alpha two\n"),
            ("src/beta.rs", b"beta two\n"),
            ("src/shared.rs", b"three\n"),
        ],
        "both shared",
    );
    repo.remove("src/alpha.rs", "remove alpha");

    repo.index();
    let output = repo.run(["related", "src/alpha.rs", "src/beta.rs"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert_eq!(
        text.matches("candidate path: src/shared.rs").count(),
        1,
        "{text}"
    );
    assert!(text.contains("co-change count: 3"), "{text}");
    assert!(text.contains("proportion: 100.0%"), "{text}");
    assert!(text.contains("supporting commits: 3"), "{text}");
    assert!(text.contains("confidence:"), "{text}");
    assert!(text.contains("basis:"), "{text}");
}

#[test]
fn related_keeps_historical_seeds_and_ignores_mass_change_noise() {
    let repo = TestRepo::new();
    repo.commit_files(&[("src/legacy.rs", b"legacy\n")], "legacy");
    repo.commit_files(
        &[
            ("src/legacy.rs", b"legacy relation\n"),
            ("src/shared.rs", b"shared\n"),
        ],
        "ordinary relation",
    );
    repo.commit_mass_change("src/legacy.rs", "src/shared.rs");
    repo.remove("src/legacy.rs", "remove legacy");
    repo.index();

    let output = repo.run(["related", "src/legacy.rs"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("candidate path: src/shared.rs"), "{text}");
    assert!(text.contains("co-change count: 1"), "{text}");
    assert!(text.contains("proportion: 100.0%"), "{text}");
}

#[test]
fn related_does_not_claim_unrelated_mass_changes() {
    let repo = TestRepo::new();
    repo.commit_files(&[("src/seed.rs", b"seed\n")], "seed");
    repo.commit_files(
        &[
            ("src/seed.rs", b"seed relation\n"),
            ("src/shared.rs", b"shared\n"),
        ],
        "ordinary relation",
    );
    repo.commit_unrelated_mass_change();
    repo.remove("src/seed.rs", "remove seed");
    repo.index();

    let output = repo.run(["related", "src/seed.rs"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(!stdout(&output).contains("mass-change commits excluded"));
}

#[test]
fn tests_filter_deleted_candidates_and_warn_about_rename_continuity() {
    let repo = TestRepo::new();
    repo.commit_files(
        &[
            ("src/widget.py", b"widget\n"),
            ("src/__tests__/widget.js", b"js test\n"),
            ("tests/test_widget.py", b"test\n"),
            ("tests/test_widget_satellite.py", b"satellite\n"),
            ("tests/test_old_widget.py", b"old\n"),
            ("tests/test_deleted_widget.py", b"deleted\n"),
        ],
        "seed test history",
    );
    repo.commit_files(
        &[
            ("src/widget.py", b"widget one\n"),
            ("src/__tests__/widget.js", b"js test one\n"),
            ("tests/test_widget.py", b"test one\n"),
            ("tests/test_widget_satellite.py", b"satellite one\n"),
            ("tests/test_old_widget.py", b"old one\n"),
            ("tests/test_deleted_widget.py", b"deleted one\n"),
        ],
        "update widget tests",
    );
    repo.remove("tests/test_deleted_widget.py", "delete obsolete test");
    repo.rename(
        "tests/test_old_widget.py",
        "tests/test_renamed_widget.py",
        "rename widget test",
    );
    repo.index();

    let output = repo.run(["tests", "src/widget.py"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("candidate path: tests/test_widget.py"),
        "{text}"
    );
    assert!(
        text.contains("candidate path: tests/test_widget_satellite.py"),
        "{text}"
    );
    assert!(
        text.contains("candidate path: src/__tests__/widget.js"),
        "{text}"
    );
    assert!(!text.contains("tests/test_renamed_widget.py"), "{text}");
    assert!(!text.contains("tests/test_deleted_widget.py"), "{text}");
    assert!(!text.contains("tests/test_old_widget.py"), "{text}");
    assert!(text.contains("beyond mirrored test name"), "{text}");
    assert!(stderr(&output).contains("warning:"), "{}", stderr(&output));
    assert!(stderr(&output).contains("rename"), "{}", stderr(&output));
}

#[test]
fn relation_rejects_empty_normalized_path() {
    let repo = TestRepo::new();
    repo.commit_files(&[("src/only.rs", b"only\n")], "only");

    let output = repo.run(["related", "./"]);

    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(stderr(&output).contains("path"), "{}", stderr(&output));
}

#[test]
fn related_does_not_double_count_merge_replays() {
    let repo = TestRepo::new();
    repo.commit_files(&[("src/seed.rs", b"seed\n")], "seed");
    git(repo.dir.path(), ["checkout", "-b", "feature"]);
    repo.commit_files(
        &[
            ("src/seed.rs", b"feature seed\n"),
            ("src/shared.rs", b"shared\n"),
        ],
        "feature relation",
    );
    git(repo.dir.path(), ["checkout", "main"]);
    git(
        repo.dir.path(),
        ["merge", "--no-ff", "feature", "-m", "merge feature"],
    );
    repo.remove("src/seed.rs", "remove seed");
    repo.index();

    let output = repo.run(["related", "src/seed.rs"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("co-change count: 1"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn relation_commands_have_fixed_empty_results() {
    let repo = TestRepo::new();
    repo.commit_files(&[("src/only.rs", b"only\n")], "only");
    repo.index();

    let related = repo.run(["related", "src/only.rs"]);
    assert_eq!(related.status.code(), Some(0));
    assert_eq!(stdout(&related), "No historical relations found.\n");

    let tests = repo.run(["tests", "src/only.rs"]);
    assert_eq!(tests.status.code(), Some(0));
    assert_eq!(stdout(&tests), "No historically related tests found.\n");
}

#[test]
fn related_matches_seed_basenames_through_the_projection() {
    let repo = TestRepo::new();
    repo.commit_files(
        &[
            ("src/widget.py", b"widget\n"),
            ("tests/test_widget.py", b"test\n"),
        ],
        "widget pair",
    );
    repo.remove("src/widget.py", "remove widget");
    repo.index();

    let output = repo.run(["related", "widget.py"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("candidate path: tests/test_widget.py"),
        "{text}"
    );
    assert!(text.contains("co-change count: 1"), "{text}");
}

#[test]
fn related_and_tests_filter_and_rank_only_scoped_cochanges() {
    let repo = TestRepo::new();
    repo.commit_files(&[("src/seed.rs", b"seed base\n")], "create seed");

    for index in 0..3 {
        let seed = format!("seed outside {index}\n");
        let source = format!("outside {index}\n");
        let test = format!("outside test {index}\n");
        repo.commit_files_at(
            &[
                ("src/seed.rs", seed.as_bytes()),
                ("src/outside.rs", source.as_bytes()),
                ("tests/test_outside.py", test.as_bytes()),
            ],
            "outside relation",
            "2001-01-02T12:00:00Z",
            "2001-01-02T12:00:00Z",
        );
    }
    repo.commit_files(&[("src/seed.rs", b"seed lower\n")], "lower boundary");
    let lower = repo.head();

    repo.commit_files_at(
        &[
            ("src/seed.rs", b"seed scoped one\n"),
            ("src/scoped.rs", b"scoped one\n"),
            ("tests/test_current.py", b"current one\n"),
        ],
        "in-range relation one",
        "1999-01-01T00:00:00Z",
        "2001-01-02T00:00:00Z",
    );
    repo.commit_files_at(
        &[
            ("src/seed.rs", b"seed outside-time\n"),
            ("src/time_only.rs", b"outside time\n"),
            ("tests/test_time_only.py", b"outside time test\n"),
        ],
        "committer outside time range",
        "2001-01-02T12:00:00Z",
        "2001-01-04T00:00:00Z",
    );
    repo.commit_files_at(
        &[
            ("src/seed.rs", b"seed scoped two\n"),
            ("src/scoped.rs", b"scoped two\n"),
            ("tests/test_current.py", b"current two\n"),
        ],
        "in-range relation two",
        "2001-01-04T00:00:00Z",
        "2001-01-04T00:30:00+01:00",
    );
    repo.commit_files_at(
        &[
            ("src/seed.rs", b"seed removed candidate\n"),
            ("tests/test_removed.py", b"removed test\n"),
        ],
        "in-range removed test",
        "2001-01-03T00:00:00Z",
        "2001-01-03T00:00:00Z",
    );
    let upper = repo.head();

    for index in 0..3 {
        let seed = format!("seed future {index}\n");
        let source = format!("future {index}\n");
        let test = format!("future test {index}\n");
        repo.commit_files_at(
            &[
                ("src/seed.rs", seed.as_bytes()),
                ("src/future.rs", source.as_bytes()),
                ("tests/test_future.py", test.as_bytes()),
            ],
            "future relation",
            "2001-01-03T12:00:00Z",
            "2001-01-03T12:00:00Z",
        );
    }
    repo.remove("tests/test_removed.py", "remove scoped test");
    repo.index();
    let cache_tip = repo.head();

    let related = repo.run([
        "related",
        "src/seed.rs",
        "--from-rev",
        lower.as_str(),
        "--to-rev",
        upper.as_str(),
        "--since",
        "2001-01-02",
        "--until",
        "2001-01-03",
        "--limit",
        "1",
        "--json",
    ]);
    assert_eq!(related.status.code(), Some(0), "{}", stderr(&related));
    let related: serde_json::Value = serde_json::from_slice(&related.stdout).unwrap();
    assert_eq!(related["matched_count"], 3);
    assert_eq!(related["truncated"], true);
    assert_eq!(related["materials"].as_array().unwrap().len(), 1);
    let candidate = &related["materials"][0];
    assert_eq!(candidate["paths"][0], "src/scoped.rs");
    assert_eq!(candidate["detail"]["co_change_count"], 2);
    assert_eq!(candidate["detail"]["supporting_count"], 2);
    let cited_subjects = candidate["citations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|citation| citation["subject"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(cited_subjects.contains(&"in-range relation one"));
    assert!(cited_subjects.contains(&"in-range relation two"));
    assert!(!cited_subjects.contains(&"outside relation"));
    let scope = &related["scope"];
    assert_eq!(scope["from_rev"], lower);
    assert_eq!(scope["to_rev"], upper);
    assert_eq!(scope["since"], "2001-01-02");
    assert_eq!(scope["until"], "2001-01-03");
    assert_eq!(scope["cache_tip"], cache_tip);

    let tests = repo.run([
        "tests",
        "src/seed.rs",
        "--from-rev",
        lower.as_str(),
        "--to-rev",
        upper.as_str(),
        "--since",
        "2001-01-02",
        "--until",
        "2001-01-03",
        "--limit",
        "1",
        "--json",
    ]);
    assert_eq!(tests.status.code(), Some(0), "{}", stderr(&tests));
    let tests: serde_json::Value = serde_json::from_slice(&tests.stdout).unwrap();
    assert_eq!(tests["matched_count"], 1);
    assert_eq!(tests["materials"].as_array().unwrap().len(), 1);
    assert_eq!(tests["materials"][0]["paths"][0], "tests/test_current.py");
    assert_eq!(tests["materials"][0]["detail"]["co_change_count"], 2);
    assert_eq!(tests["scope"]["cache_tip"], cache_tip);
    let tests_output = serde_json::to_string(&tests).unwrap();
    for excluded in [
        "tests/test_outside.py",
        "tests/test_time_only.py",
        "tests/test_future.py",
        "tests/test_removed.py",
    ] {
        assert!(!tests_output.contains(excluded), "{tests_output}");
    }

    for command in ["related", "tests"] {
        for (flag, value) in [
            ("--from-rev", lower.as_str()),
            ("--to-rev", upper.as_str()),
            ("--since", "2001-01-02"),
            ("--until", "2001-01-03"),
        ] {
            let output = repo.run([command, "src/seed.rs", flag, value]);
            assert_eq!(
                output.status.code(),
                Some(0),
                "{command} {flag}: {}",
                stderr(&output)
            );
        }
    }
    let implicit_tip = repo.run([
        "related",
        "src/seed.rs",
        "--from-rev",
        lower.as_str(),
        "--json",
    ]);
    let implicit_tip: serde_json::Value = serde_json::from_slice(&implicit_tip.stdout).unwrap();
    assert_eq!(implicit_tip["scope"]["to_rev"], cache_tip);
    assert_eq!(implicit_tip["scope"]["cache_tip"], cache_tip);

    let unscoped = repo.run(["related", "src/seed.rs", "--json"]);
    let unscoped: serde_json::Value = serde_json::from_slice(&unscoped.stdout).unwrap();
    assert!(unscoped.get("scope").is_none());
}

#[test]
fn scoped_relation_queries_report_empty_scope_and_reject_reversed_time() {
    let repo = TestRepo::new();
    repo.commit_files(&[("src/only.rs", b"only\n")], "only");
    repo.index();
    let tip = repo.head();

    for (command, empty_message) in [
        ("related", "No historical relations found."),
        ("tests", "No historically related tests found."),
    ] {
        let json = repo.run([command, "src/only.rs", "--from-rev", tip.as_str(), "--json"]);
        assert_eq!(json.status.code(), Some(0), "{}", stderr(&json));
        let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
        assert_eq!(value["matched_count"], 0);
        assert_eq!(value["materials"].as_array().unwrap().len(), 0);
        assert_eq!(value["scope"]["from_rev"], tip);
        assert_eq!(value["scope"]["to_rev"], tip);
        assert_eq!(value["scope"]["cache_tip"], tip);

        let human = repo.run([command, "src/only.rs", "--from-rev", tip.as_str()]);
        assert_eq!(human.status.code(), Some(0), "{}", stderr(&human));
        let output = stdout(&human);
        assert!(
            output.starts_with(&format!("Scope: commits reachable from {tip};")),
            "{output}"
        );
        assert!(output.contains(empty_message), "{output}");

        let invalid = repo.run([
            command,
            "src/only.rs",
            "--since",
            "2001-01-03",
            "--until",
            "2001-01-02",
        ]);
        assert_eq!(invalid.status.code(), Some(2), "{}", stderr(&invalid));
        assert!(
            stderr(&invalid).contains("--since must not be later than --until"),
            "{}",
            stderr(&invalid)
        );
    }
}

#[test]
fn related_and_tests_help_document_scope_rules() {
    let repo = TestRepo::new();
    for command in ["related", "tests"] {
        let output = repo.run([command, "--help"]);
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        let help = stdout(&output);
        for expected in [
            "--from-rev",
            "--to-rev",
            "--since",
            "--until",
            "committer time",
            "UTC calendar day",
            "RFC 3339",
            "published cache tip",
            "before ranking and `--limit`",
        ] {
            assert!(help.contains(expected), "missing {expected:?} in:\n{help}");
        }
        if command == "tests" {
            assert!(help.contains("current test paths"), "{help}");
            assert!(help.contains("scope narrows historical support"), "{help}");
        }
    }
}

#[test]
fn patterns_discover_joint_subsets_and_include_seed_only_denominator() {
    let repo = TestRepo::new();
    for index in 0..3 {
        let content = format!("version {index}\n");
        let incidental = format!("extra/{index}.txt");
        repo.commit_files(
            &[
                ("A", content.as_bytes()),
                ("B", content.as_bytes()),
                ("C", content.as_bytes()),
                (&incidental, b"extra"),
            ],
            "joint change",
        );
    }
    repo.commit_files(&[("A", b"alone\n")], "seed only");
    repo.index();
    let output = repo.run(["related", "A", "A", "--patterns", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["kind"], "patterns");
    assert_eq!(value["eligible_seed_commits"], 4);
    assert_eq!(value["patterns"].as_array().unwrap().len(), 1);
    let pattern = &value["patterns"][0];
    assert_eq!(pattern["support_count"], 3);
    assert_eq!(pattern["proportion"], 0.75);
    let members = pattern["members"].as_array().unwrap();
    assert_eq!(members.len(), 3);
    assert_eq!(members[0]["path"], "A");
    assert_eq!(members[0]["seed"], true);
    assert_eq!(members[1]["seed"], false);
    assert_eq!(pattern["citations"].as_array().unwrap().len(), 3);
    let human = repo.run(["related", "A", "--patterns"]);
    assert!(stdout(&human).contains("all seeds"));
    assert!(stdout(&human).contains("75.0%"));
}

#[test]
fn patterns_require_all_seeds_and_keep_only_closed_combinations() {
    let repo = TestRepo::new();
    for index in 0..3 {
        let content = format!("{index}\n");
        repo.commit_files(
            &[
                ("A", content.as_bytes()),
                ("B", content.as_bytes()),
                ("C", content.as_bytes()),
                ("D", content.as_bytes()),
            ],
            "complete",
        );
    }
    repo.commit_files(&[("A", b"only A"), ("X", b"X")], "partial");
    repo.index();
    let output = repo.run(["related", "A", "B", "--patterns", "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["eligible_seed_commits"], 3);
    assert_eq!(value["patterns"].as_array().unwrap().len(), 1);
    assert_eq!(value["patterns"][0]["members"].as_array().unwrap().len(), 4);
    assert_eq!(value["patterns"][0]["proportion"], 1.0);
    let absent = repo.run(["related", "A", "missing", "--patterns", "--json"]);
    let absent: serde_json::Value = serde_json::from_slice(&absent.stdout).unwrap();
    assert_eq!(absent["eligible_seed_commits"], 0);
    assert_eq!(absent["patterns"], serde_json::json!([]));
}

#[test]
fn patterns_scope_before_counts_keep_historical_members_and_cap_references() {
    let repo = TestRepo::new();
    repo.commit_files(
        &[("A", b"initial"), ("B", b"initial"), ("C", b"initial")],
        "start",
    );
    let start = repo.head();
    for index in 0..6 {
        let content = format!("{index}\n");
        repo.commit_files(
            &[
                ("A", content.as_bytes()),
                ("B", content.as_bytes()),
                ("C", content.as_bytes()),
            ],
            "joint",
        );
    }
    let end = repo.head();
    repo.remove("C", "delete C");
    repo.index();
    let output = repo.run([
        "related",
        "A",
        "--patterns",
        "--from-rev",
        &start,
        "--to-rev",
        &end,
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["eligible_seed_commits"], 6);
    assert_eq!(value["patterns"][0]["support_count"], 6);
    assert_eq!(
        value["patterns"][0]["citations"].as_array().unwrap().len(),
        5
    );
    assert_eq!(value["patterns"][0]["references_not_shown"], 1);
    assert_eq!(
        value["patterns"][0]["members"][2]["exists_at_target"],
        false
    );
    assert_eq!(value["target_revision"], repo.head());
    assert_eq!(value["scope"]["to_rev"], end);
}

#[test]
fn patterns_minimum_support_and_cli_validation() {
    let repo = TestRepo::new();
    for index in 0..2 {
        let content = format!("{index}");
        repo.commit_files(
            &[
                ("A", content.as_bytes()),
                ("B", content.as_bytes()),
                ("C", content.as_bytes()),
            ],
            "joint",
        );
    }
    repo.index();
    let empty = repo.run(["related", "A", "--patterns", "--json"]);
    let empty: serde_json::Value = serde_json::from_slice(&empty.stdout).unwrap();
    assert_eq!(empty["patterns"], serde_json::json!([]));
    let output = repo.run(["related", "A", "--patterns", "--min-support", "2", "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["patterns"][0]["support_count"], 2);
    for args in [
        vec!["related", "A", "--min-support", "2"],
        vec!["related", "A", "--patterns", "--min-support", "1"],
        vec!["related", "A", "--patterns", "--limit", "0"],
        vec!["related", "../A", "--patterns"],
        vec!["related", "A", "--patterns", "--github-links"],
    ] {
        assert_eq!(repo.run(args).status.code(), Some(2));
    }
}

#[test]
fn patterns_rank_distinct_groups_before_limit_and_ignore_mass_changes() {
    let repo = TestRepo::new();
    for (prefix, count) in [("B", 4), ("D", 3)] {
        for index in 0..count {
            let content = format!("{prefix}{index}");
            let peer = format!("{prefix}2");
            repo.commit_files(
                &[
                    ("A", content.as_bytes()),
                    (prefix, content.as_bytes()),
                    (&peer, content.as_bytes()),
                ],
                "joint",
            );
        }
    }
    repo.commit_mass_change("A", "B");
    repo.index();
    let output = repo.run(["related", "A", "--patterns", "--limit", "1", "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["eligible_seed_commits"], 7);
    assert_eq!(value["excluded_mass_changes"], 1);
    assert_eq!(value["matched_count"], 2);
    assert_eq!(value["returned_count"], 1);
    assert_eq!(value["patterns"][0]["support_count"], 4);
    assert_eq!(value["patterns"][0]["members"][1]["path"], "B");
    assert_eq!(
        repo.run(["related", "A", "--patterns", "--json"]).stdout,
        repo.run(["related", "A", "--patterns", "--json"]).stdout
    );
}

#[test]
fn patterns_exclude_merge_replays_and_intersect_time_scope() {
    let repo = TestRepo::new();
    repo.commit_files_at(
        &[("A", b"0"), ("B", b"0"), ("C", b"0")],
        "start",
        "2025-01-01T00:00:00Z",
        "2025-01-01T00:00:00Z",
    );
    git(repo.dir.path(), ["checkout", "-b", "feature"]);
    for index in 1..3 {
        let content = format!("{index}");
        repo.commit_files_at(
            &[
                ("A", content.as_bytes()),
                ("B", content.as_bytes()),
                ("C", content.as_bytes()),
            ],
            "joint",
            "2025-01-02T00:00:00Z",
            "2025-01-02T00:00:00Z",
        );
    }
    git(repo.dir.path(), ["checkout", "main"]);
    git(
        repo.dir.path(),
        ["merge", "--no-ff", "feature", "-m", "merge feature"],
    );
    repo.index();
    let output = repo.run(["related", "A", "--patterns", "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["eligible_seed_commits"], 3);
    assert_eq!(value["excluded_merges"], 1);
    assert_eq!(value["patterns"][0]["support_count"], 3);
    let scoped = repo.run([
        "related",
        "A",
        "--patterns",
        "--min-support",
        "2",
        "--since",
        "2025-01-02",
        "--until",
        "2025-01-02",
        "--json",
    ]);
    let scoped: serde_json::Value = serde_json::from_slice(&scoped.stdout).unwrap();
    assert_eq!(scoped["eligible_seed_commits"], 2);
    assert_eq!(scoped["excluded_merges"], 0);
    assert_eq!(scoped["patterns"][0]["support_count"], 2);
}

#[test]
fn patterns_include_fifty_paths_but_exclude_fifty_one() {
    let repo = TestRepo::new();
    for (index, count) in [50, 50, 51].into_iter().enumerate() {
        let version = repo.run(["related", "A", "--patterns"]); // no cache: must not build it
        assert_eq!(version.status.code(), Some(1));
        let marker = format!("{index}");
        let files = (0..count)
            .map(|index| {
                (
                    if index == 0 {
                        "A".to_owned()
                    } else {
                        format!("file/{index}")
                    },
                    marker.as_bytes().to_vec(),
                )
            })
            .collect::<Vec<_>>();
        let borrowed = files
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
            .collect::<Vec<_>>();
        repo.commit_files(&borrowed, "boundary");
    }
    repo.index();
    let output = repo.run(["related", "A", "--patterns", "--min-support", "2", "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["eligible_seed_commits"], 2);
    assert_eq!(value["excluded_mass_changes"], 1);
    assert_eq!(value["patterns"][0]["support_count"], 2);
    assert_eq!(
        value["patterns"][0]["members"].as_array().unwrap().len(),
        50
    );
}

#[test]
fn patterns_retain_smaller_closed_groups_and_scope_can_change_closedness() {
    let repo = TestRepo::new();
    for index in 0..3 {
        let content = format!("{index}");
        repo.commit_files(
            &[
                ("A", content.as_bytes()),
                ("B", content.as_bytes()),
                ("C", content.as_bytes()),
                ("D", content.as_bytes()),
            ],
            "large",
        );
    }
    let end = repo.head();
    repo.commit_files(
        &[("A", b"small"), ("B", b"small"), ("C", b"small")],
        "small",
    );
    repo.index();
    let full = repo.run(["related", "A", "--patterns", "--json"]);
    let full: serde_json::Value = serde_json::from_slice(&full.stdout).unwrap();
    assert_eq!(full["matched_count"], 2);
    assert_eq!(full["patterns"][0]["support_count"], 4);
    assert_eq!(full["patterns"][0]["members"].as_array().unwrap().len(), 3);
    assert_eq!(full["patterns"][1]["support_count"], 3);
    let scoped = repo.run(["related", "A", "--patterns", "--to-rev", &end, "--json"]);
    let scoped: serde_json::Value = serde_json::from_slice(&scoped.stdout).unwrap();
    assert_eq!(scoped["matched_count"], 1);
    assert_eq!(
        scoped["patterns"][0]["members"].as_array().unwrap().len(),
        4
    );
}

#[test]
fn patterns_do_not_fabricate_joint_support_or_fold_case_or_use_worktree_existence() {
    let repo = TestRepo::new();
    for index in 0..3 {
        let content = format!("{index}");
        repo.commit_files(
            &[("A", content.as_bytes()), ("B", content.as_bytes())],
            "pair B",
        );
        repo.commit_files(
            &[
                ("A", format!("C{index}").as_bytes()),
                ("C", content.as_bytes()),
            ],
            "pair C",
        );
    }
    repo.index();
    let output = repo.run(["related", "A", "--patterns", "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["matched_count"], 0);
    let case = repo.run(["related", "a", "--patterns", "--json"]);
    let case: serde_json::Value = serde_json::from_slice(&case.stdout).unwrap();
    assert_eq!(case["eligible_seed_commits"], 0);
    for index in 0..3 {
        let content = format!("joint {index}");
        repo.commit_files(
            &[
                ("A", content.as_bytes()),
                ("B", content.as_bytes()),
                ("C", content.as_bytes()),
            ],
            "joint",
        );
    }
    repo.index();
    fs::remove_file(repo.dir.path().join("C")).unwrap();
    let output = repo.run(["related", "A", "--patterns", "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["patterns"][0]["members"][2]["exists_at_target"], true);
}

#[test]
fn patterns_discover_all_closed_groups_without_budget_cancellation() {
    let repo = TestRepo::new();
    // Each observation omits a different item: intersections grow exponentially.
    for omitted in 0..13 {
        let marker = format!("{omitted}");
        let mut files = vec![("A".to_owned(), marker.as_bytes().to_vec())];
        for item in 0..13 {
            if item != omitted {
                files.push((format!("item/{item}"), marker.as_bytes().to_vec()));
            }
        }
        let borrowed = files
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
            .collect::<Vec<_>>();
        repo.commit_files(&borrowed, "omit one");
    }
    repo.index();
    let output = repo.run(["related", "A", "--patterns", "--limit", "1", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    // All subsets of 2..=10 items qualify: 13 - subset size supporting commits.
    assert_eq!(value["matched_count"], 8086);
    assert_eq!(value["returned_count"], 1);
    assert_eq!(value["eligible_seed_commits"], 13);
    assert_eq!(value["patterns"][0]["support_count"], 11);
}

#[test]
fn patterns_disclose_unscoped_history_and_order_count_recency_then_paths() {
    let repo = TestRepo::new();
    for (path, day) in [("old", "01"), ("z", "02"), ("b", "02")] {
        for index in 0..3 {
            let marker = format!("{path}{index}");
            let date = format!("2024-01-{day}T00:00:00Z");
            repo.commit_files_at(
                &[
                    ("A", marker.as_bytes()),
                    ("common", marker.as_bytes()),
                    (path, marker.as_bytes()),
                ],
                "group",
                &date,
                &date,
            );
        }
    }
    repo.index();
    let human = repo.run(["related", "A", "--patterns"]);
    assert!(stdout(&human).contains("History: all available published-cache commits"));
    let output = repo.run(["related", "A", "--patterns", "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["history_coverage"],
        "available published-cache commits only"
    );
    let patterns = value["patterns"].as_array().unwrap();
    assert_eq!(patterns.len(), 3);
    for (pattern, path) in patterns.iter().zip(["b", "z", "old"]) {
        assert_eq!(pattern["support_count"], 3);
        assert!(
            pattern["members"]
                .as_array()
                .unwrap()
                .iter()
                .any(|member| member["path"] == path)
        );
    }
}
