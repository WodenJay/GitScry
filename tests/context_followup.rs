mod support;

use serde_json::Value;
use std::fs;
use support::{TestRepo, git};

#[test]
fn context_returns_repeated_historical_followup_with_traceable_statistics() {
    let repo = TestRepo::new();
    let (_alpha, _beta, first_later, second_later) = complete_context_fixture(&repo, false);
    let report = json(repo.run(["context", "--json"]));
    let followup = report["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|suggestion| suggestion["category"] == "historical_followup")
        .expect("qualifying candidate should be returned as historical follow-up");

    assert_eq!(followup["path"], "candidate.rs");
    assert_eq!(followup["historical_followup"]["supporting_origins"], 2);
    assert_eq!(followup["historical_followup"]["complete_origins"], 2);
    assert_eq!(followup["historical_followup"]["independent_chains"], 2);
    assert_eq!(followup["historical_followup"]["baseline_occurrences"], 2);
    assert!(
        followup["historical_followup"]["baseline_sample_size"]
            .as_u64()
            .unwrap()
            >= 2
    );

    let chains = followup["historical_followup"]["chains"]
        .as_array()
        .unwrap();
    assert_eq!(chains.len(), 2);
    assert_ne!(chains[0]["origin_oid"], chains[1]["origin_oid"]);
    assert_ne!(chains[0]["later_oid"], chains[1]["later_oid"]);
    let later_oids = [first_later, second_later];
    assert!(
        chains
            .iter()
            .all(|chain| later_oids.contains(&chain["later_oid"].as_str().unwrap().to_owned()))
    );
    let statistics = &followup["historical_followup"];
    let support = statistics["supporting_origins"].as_u64().unwrap();
    let complete = statistics["complete_origins"].as_u64().unwrap();
    let baseline = statistics["baseline_occurrences"].as_u64().unwrap();
    let sample_size = statistics["baseline_sample_size"].as_u64().unwrap();
    assert!(support * 2 >= complete);
    assert!(support * sample_size >= 2 * baseline * complete);

    let clipped = json(repo.run(["context", "--json", "--until", "2025-02-07"]));
    assert!(
        clipped["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|suggestion| suggestion["category"] != "historical_followup"),
        "clipped observation windows must not support a follow-up"
    );
    let staged = json(repo.run(["context", "--staged", "--json"]));
    assert!(
        staged["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|suggestion| suggestion["category"] != "historical_followup"),
        "staged context must not use unstaged content origins"
    );

    let rendered = repo.run(["context"]);
    assert!(rendered.status.success());
    let rendered = String::from_utf8(rendered.stdout).unwrap();
    assert!(rendered.contains(&format!(
        "Historical follow-up: supporting origins 2/2, 2 independent chains; candidate baseline 2/{sample_size}"
    )));
    assert!(rendered.contains(&format!(
        "Historical chain: {} -> {}",
        chains[0]["origin_oid"].as_str().unwrap(),
        chains[0]["later_oid"].as_str().unwrap()
    )));
}

#[test]
fn context_reports_unbudgeted_followup_coverage_without_refreshing_cache() {
    let repo = TestRepo::new();
    complete_context_fixture(&repo, false);
    let published_tip = repo.head();
    empty_commit_at(
        &repo,
        "unpublished later commit",
        "2025-02-12T00:00:00+0000",
    );

    let report = json(repo.run(["context", "--json"]));
    let coverage = &report["historical_followup_coverage"];
    assert_eq!(report["cache_tip"], published_tip);
    assert_eq!(coverage["budget"], Value::Null);
    assert_eq!(coverage["limited"], false);
    // The newer HEAD is unpublished; read-only context cannot check relationships to it.

    let rendered = repo.run(["context"]);
    let rendered = String::from_utf8(rendered.stdout).unwrap();
    assert!(rendered.contains("Historical follow-up coverage:"));
    assert!(rendered.contains("budget: unlimited; limited: false"));

    let budgeted = json(repo.run(["context", "--max-followup-checks", "1", "--json"]));
    assert_eq!(budgeted["cache_tip"], published_tip);
    assert_eq!(budgeted["historical_followup_coverage"]["budget"], 1);
}

#[test]
fn context_discloses_incomplete_published_history_without_refreshing() {
    let repo = TestRepo::new();
    commit_at(
        &repo,
        &[("current.rs", "fn before() {}\n")],
        "cached base",
        "2025-01-01T00:00:00+0000",
    );
    let published_tip = repo.head();
    fs::write(
        repo.dir.path().join(".git/shallow"),
        format!("{published_tip}\n"),
    )
    .unwrap();
    repo.index();
    fs::write(repo.dir.path().join("current.rs"), "fn after() {}\n").unwrap();

    let report = json(repo.run(["context", "--json"]));

    assert_eq!(report["cache_tip"], published_tip);
    assert!(
        report["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| {
                warning
                    .as_str()
                    .unwrap()
                    .contains("incomplete history coverage for")
            }),
        "{report}"
    );
}

#[test]
fn budget_covers_candidate_origin_descendants_before_paths_are_known() {
    let repo = TestRepo::new();
    commit_at(
        &repo,
        &[("current.rs", "fn placeholder() {}\n")],
        "base",
        "2025-01-01T00:00:00+0000",
    );
    let alpha = "fn alpha_feature() { let outcome = identityAlpha(); }\n";
    commit_at(
        &repo,
        &[("current.rs", alpha)],
        "content origin alpha",
        "2025-02-01T00:00:00+0000",
    );
    let beta = format!("{alpha}fn beta_feature() {{ let outcome = identityBeta(); }}\n");
    commit_at(
        &repo,
        &[("current.rs", &beta)],
        "content origin beta",
        "2025-02-02T00:00:00+0000",
    );
    commit_at(
        &repo,
        &[("current.rs", &format!("{beta}fn later_change() {{}}\n"))],
        "same-path descendant",
        "2025-02-04T00:00:00+0000",
    );
    commit_at(
        &repo,
        &[("current.rs", "fn placeholder() {}\n")],
        "restore current baseline",
        "2025-02-10T00:00:00+0000",
    );
    repo.index();
    fs::write(repo.dir.path().join("current.rs"), beta).unwrap();

    let report = json(repo.run(["context", "--max-followup-checks", "1", "--json"]));
    let coverage = &report["historical_followup_coverage"];
    assert_eq!(coverage["relationship_checks"], 1, "{report}");
    assert_eq!(coverage["candidate_checks"], 1, "{report}");
    assert_eq!(coverage["baseline_checks"], 0, "{report}");
    assert_eq!(coverage["limited"], true, "{report}");
    assert!(
        report["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|suggestion| suggestion["category"] != "historical_followup"),
        "{report}"
    );
}

#[test]
fn context_budget_exhaustion_discards_incomplete_content_origin_window() {
    let repo = TestRepo::new();
    complete_context_fixture(&repo, false);

    let report = json(repo.run(["context", "--json", "--max-followup-checks", "1"]));
    let coverage = &report["historical_followup_coverage"];
    assert_eq!(coverage["relationship_checks"], 1);
    assert_eq!(coverage["candidate_checks"], 1);
    assert_eq!(coverage["baseline_checks"], 0);
    assert_eq!(coverage["budget"], 1);
    assert_eq!(coverage["limited"], true);
    assert!(
        report["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|suggestion| suggestion["category"] != "historical_followup")
    );
    assert!(
        report["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|suggestion| suggestion["category"] == "historical_change")
    );
    assert!(
        report["limitations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|limitation| limitation.as_str().unwrap().contains("follow-up budget"))
    );
    let human = repo.run(["context", "--max-followup-checks", "1"]);
    assert!(human.status.success());
    let human = String::from_utf8(human.stdout).unwrap();
    assert!(human.contains("budget: 1; limited: true"));
    assert!(human.contains("Historical follow-up budget limited coverage"));
}

#[test]
fn context_budget_exhaustion_discards_incomplete_baseline_window() {
    let repo = TestRepo::new();
    complete_context_fixture(&repo, false);

    let report = json(repo.run(["context", "--json", "--max-followup-checks", "45"]));
    let coverage = &report["historical_followup_coverage"];
    assert_eq!(coverage["relationship_checks"], 45, "{coverage:?}");
    assert_eq!(coverage["candidate_checks"], 4, "{coverage:?}");
    assert_eq!(coverage["baseline_checks"], 41);
    assert_eq!(coverage["budget"], 45);
    assert_eq!(coverage["limited"], true);
    let followup = report["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|suggestion| suggestion["category"] == "historical_followup")
        .unwrap_or_else(|| panic!("completed windows still support the follow-up: {report}"));
    let statistics = &followup["historical_followup"];
    assert_eq!(statistics["complete_origins"], 2);
    assert_eq!(statistics["baseline_occurrences"], 0);
    assert_eq!(statistics["baseline_sample_size"], 8);

    let repeated = json(repo.run(["context", "--json", "--max-followup-checks", "45"]));
    assert_eq!(
        repeated["historical_followup_coverage"],
        report["historical_followup_coverage"]
    );
    assert_eq!(repeated["suggestions"], report["suggestions"]);
}

#[test]
fn context_followup_budget_requires_positive_value_and_is_documented() {
    let repo = TestRepo::new();
    let help = repo.run(["context", "--help"]);
    assert!(help.status.success());
    let help = String::from_utf8_lossy(&help.stdout);
    for expected in [
        "--max-followup-checks",
        "no maximum or timeout applies by default",
        "One check is a distinct eligible, non-merge descendant commit",
        "Stronger content origins run first",
        "baseline origins use stable graph order",
        "completed candidate window reused for baseline statistics is counted once",
    ] {
        assert!(help.contains(expected), "missing {expected:?} in:\n{help}");
    }

    let invalid = repo.run(["context", "--max-followup-checks", "0"]);
    assert!(!invalid.status.success());
    assert!(!invalid.stderr.is_empty());
}

#[test]
fn context_rejects_two_origins_reaching_only_one_later_change() {
    let repo = TestRepo::new();
    commit_at(
        &repo,
        &[
            ("current.rs", "fn placeholder() {}\n"),
            ("candidate.rs", "fn candidate_start() {}\n"),
            (
                "historical-a.rs",
                "fn alpha_feature() { let outcome = false; }\n",
            ),
            (
                "historical-b.rs",
                "fn beta_feature() { let outcome = false; }\n",
            ),
        ],
        "base",
        "2025-01-01T00:00:00+0000",
    );
    let alpha = "fn alpha_feature() { let outcome = identityAlpha(); }\n";
    let beta = "fn beta_feature() { let outcome = identityBeta(); }\n";
    commit_at(
        &repo,
        &[("historical-a.rs", alpha)],
        "content origin alpha",
        "2025-02-01T00:00:00+0000",
    );
    commit_at(
        &repo,
        &[("historical-b.rs", beta)],
        "content origin beta",
        "2025-02-02T00:00:00+0000",
    );
    commit_at(
        &repo,
        &[("candidate.rs", "fn only_candidate_followup() {}\n")],
        "single candidate follow-up",
        "2025-02-03T00:00:00+0000",
    );
    empty_commit_at(
        &repo,
        "observation windows complete",
        "2025-02-10T00:00:00+0000",
    );
    repo.index();
    fs::write(repo.dir.path().join("current.rs"), format!("{alpha}{beta}")).unwrap();

    let report = json(repo.run(["context", "--json"]));
    assert!(
        report["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|suggestion| { suggestion["category"] != "historical_followup" })
    );
}

#[test]
fn context_rejects_followup_that_does_not_outperform_candidate_baseline() {
    let repo = TestRepo::new();
    commit_at(
        &repo,
        &[
            ("current.rs", "fn placeholder() {}\n"),
            ("candidate.rs", "fn candidate_start() {}\n"),
            (
                "historical-a.rs",
                "fn alpha_feature() { let outcome = false; }\n",
            ),
            (
                "historical-b.rs",
                "fn beta_feature() { let outcome = false; }\n",
            ),
        ],
        "base",
        "2025-01-01T00:00:00+0000",
    );
    let alpha = "fn alpha_feature() { let outcome = identityAlpha(); }\n";
    let beta = "fn beta_feature() { let outcome = identityBeta(); }\n";
    commit_at(
        &repo,
        &[("historical-a.rs", alpha)],
        "content origin alpha",
        "2025-02-01T00:00:00+0000",
    );
    commit_at(
        &repo,
        &[("baseline.rs", "fn baseline_origin() {}\n")],
        "baseline origin",
        "2025-02-01T12:00:00+0000",
    );
    commit_at(
        &repo,
        &[("historical-b.rs", beta)],
        "content origin beta",
        "2025-02-02T00:00:00+0000",
    );
    commit_at(
        &repo,
        &[("candidate.rs", "fn candidate_followup_one() {}\n")],
        "first candidate follow-up",
        "2025-02-03T00:00:00+0000",
    );
    commit_at(
        &repo,
        &[("candidate.rs", "fn candidate_followup_two() {}\n")],
        "second candidate follow-up",
        "2025-02-04T00:00:00+0000",
    );
    empty_commit_at(
        &repo,
        "observation windows complete",
        "2025-02-11T00:00:00+0000",
    );
    repo.index();
    fs::write(repo.dir.path().join("current.rs"), format!("{alpha}{beta}")).unwrap();

    let report = json(repo.run(["context", "--json"]));
    assert!(
        report["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|suggestion| suggestion["category"] != "historical_followup"),
        "frequent candidate changes should fail the two-times baseline gate"
    );
}

#[test]
fn context_does_not_treat_origins_before_a_merged_file_as_independent_support() {
    let repo = TestRepo::new();
    commit_at(
        &repo,
        &[
            ("current.rs", "fn placeholder() {}\n"),
            (
                "historical-a.rs",
                "fn alpha_feature() { let outcome = false; }\n",
            ),
            (
                "historical-b.rs",
                "fn beta_feature() { let outcome = false; }\n",
            ),
        ],
        "base without candidate",
        "2025-01-01T00:00:00+0000",
    );
    for day in 2..=11 {
        commit_at(
            &repo,
            &[(
                &format!("decoy-{day}.rs"),
                &format!("fn unrelated_{day}() {{}}\n"),
            )],
            &format!("unrelated {day}"),
            &format!("2025-01-{day:02}T00:00:00+0000"),
        );
    }
    git(repo.dir.path(), ["checkout", "-b", "followup-side"]);
    let alpha = "fn alpha_feature() { let outcome = identityAlpha(); }\n";
    let beta = "fn beta_feature() { let outcome = identityBeta(); }\n";
    commit_at(
        &repo,
        &[
            ("historical-a.rs", alpha),
            ("candidate.rs", "fn candidate_origin_one() {}\n"),
        ],
        "content origin alpha adds candidate",
        "2025-02-01T00:00:00+0000",
    );
    commit_at(
        &repo,
        &[
            ("historical-b.rs", beta),
            ("candidate.rs", "fn candidate_origin_two() {}\n"),
        ],
        "content origin beta changes candidate",
        "2025-02-02T00:00:00+0000",
    );
    git(repo.dir.path(), ["checkout", "main"]);
    let merge = support::git_command(repo.dir.path())
        .args(["merge", "--no-ff", "--no-edit", "followup-side"])
        .env("GIT_AUTHOR_DATE", "2025-02-04T00:00:00+0000")
        .env("GIT_COMMITTER_DATE", "2025-02-04T00:00:00+0000")
        .output()
        .unwrap();
    assert!(
        merge.status.success(),
        "git merge failed: {}",
        String::from_utf8_lossy(&merge.stderr)
    );
    assert!(
        support::git_stdout(
            repo.dir.path(),
            [
                "diff",
                "--name-status",
                "HEAD^1",
                "HEAD",
                "--",
                "candidate.rs"
            ]
        )
        .starts_with("A\tcandidate.rs"),
        "merge should add candidate relative to its first parent"
    );
    commit_at(
        &repo,
        &[("candidate.rs", "fn candidate_followup_one() {}\n")],
        "first post-merge candidate change",
        "2025-02-05T00:00:00+0000",
    );
    commit_at(
        &repo,
        &[("candidate.rs", "fn candidate_followup_two() {}\n")],
        "second post-merge candidate change",
        "2025-02-06T00:00:00+0000",
    );
    empty_commit_at(
        &repo,
        "observation window complete",
        "2025-02-11T00:00:00+0000",
    );
    repo.index();
    fs::write(repo.dir.path().join("current.rs"), format!("{alpha}{beta}")).unwrap();

    let report = json(repo.run(["context", "--json"]));
    assert!(
        report["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|suggestion| suggestion["category"] != "historical_followup"),
        "origins that changed this file incarnation must not support it"
    );
}

#[test]
fn context_traces_candidate_incarnation_through_merge_without_path_delta() {
    let repo = TestRepo::new();
    commit_at(
        &repo,
        &[
            ("current.rs", "fn placeholder() {}\n"),
            ("candidate.rs", "fn candidate_start() {}\n"),
            (
                "historical-a.rs",
                "fn alpha_feature() { let outcome = false; }\n",
            ),
            (
                "historical-b.rs",
                "fn beta_feature() { let outcome = false; }\n",
            ),
        ],
        "base",
        "2025-01-01T00:00:00+0000",
    );
    for day in 2..=11 {
        commit_at(
            &repo,
            &[(
                &format!("decoy-{day}.rs"),
                &format!("fn unrelated_{day}() {{}}\n"),
            )],
            &format!("unrelated {day}"),
            &format!("2025-01-{day:02}T00:00:00+0000"),
        );
    }
    git(repo.dir.path(), ["branch", "followup-side"]);
    let alpha = "fn alpha_feature() { let outcome = identityAlpha(); }\n";
    let beta = "fn beta_feature() { let outcome = identityBeta(); }\n";
    commit_at(
        &repo,
        &[
            ("historical-a.rs", alpha),
            ("candidate.rs", "fn candidate_origin_one() {}\n"),
        ],
        "content origin alpha changes candidate on first parent",
        "2025-02-01T00:00:00+0000",
    );
    commit_at(
        &repo,
        &[
            ("historical-b.rs", beta),
            ("candidate.rs", "fn candidate_origin_two() {}\n"),
        ],
        "content origin beta changes candidate on first parent",
        "2025-02-02T00:00:00+0000",
    );
    git(repo.dir.path(), ["checkout", "followup-side"]);
    commit_at(
        &repo,
        &[("candidate.rs", "fn candidate_side_branch_change() {}\n")],
        "side branch candidate change",
        "2025-02-03T00:00:00+0000",
    );
    git(repo.dir.path(), ["checkout", "main"]);
    let merge = support::git_command(repo.dir.path())
        .args([
            "merge",
            "--no-ff",
            "--no-edit",
            "-s",
            "ours",
            "followup-side",
        ])
        .env("GIT_AUTHOR_DATE", "2025-02-04T00:00:00+0000")
        .env("GIT_COMMITTER_DATE", "2025-02-04T00:00:00+0000")
        .output()
        .unwrap();
    assert!(
        merge.status.success(),
        "git merge failed: {}",
        String::from_utf8_lossy(&merge.stderr)
    );
    assert!(
        support::git_stdout(
            repo.dir.path(),
            [
                "diff",
                "--name-status",
                "HEAD^1",
                "HEAD",
                "--",
                "candidate.rs"
            ]
        )
        .is_empty(),
        "merge should not have a candidate-path delta"
    );
    commit_at(
        &repo,
        &[("candidate.rs", "fn candidate_followup_one() {}\n")],
        "first post-merge candidate change",
        "2025-02-05T00:00:00+0000",
    );
    commit_at(
        &repo,
        &[("candidate.rs", "fn candidate_followup_two() {}\n")],
        "second post-merge candidate change",
        "2025-02-06T00:00:00+0000",
    );
    empty_commit_at(
        &repo,
        "observation window complete",
        "2025-02-11T00:00:00+0000",
    );
    repo.index();
    fs::write(repo.dir.path().join("current.rs"), format!("{alpha}{beta}")).unwrap();

    let report = json(repo.run(["context", "--json"]));
    assert!(
        report["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|suggestion| suggestion["category"] != "historical_followup"),
        "origins changing either parent history must be excluded from support"
    );
}

#[test]
fn context_rejects_symlink_candidate_materialized_as_regular_file() {
    let repo = TestRepo::new();
    complete_context_fixture(&repo, true);
    let candidate = repo.dir.path().join("candidate.rs");
    assert!(
        candidate.is_file(),
        "core.symlinks=false should materialize a regular file"
    );
    assert_eq!(
        support::git_stdout(repo.dir.path(), ["ls-tree", "HEAD", "--", "candidate.rs"])
            .split_whitespace()
            .next(),
        Some("120000"),
        "HEAD should still record a symlink"
    );
    let report = json(repo.run(["context", "--json"]));
    assert!(
        report["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|suggestion| suggestion["category"] != "historical_followup"),
        "symlinks must not be emitted as regular-file candidates"
    );
}

#[test]
fn context_does_not_link_scoped_origins_to_a_recreated_current_file() {
    let repo = TestRepo::new();
    let (alpha, beta, _, _) = complete_context_fixture(&repo, false);
    let scoped_revision = repo.head();
    let scoped_report = json(repo.run(["context", "--json", "--to-rev", &scoped_revision]));
    assert!(
        scoped_report["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|suggestion| suggestion["category"] == "historical_followup"),
        "fixture should qualify at the selected historical revision"
    );
    git(repo.dir.path(), ["checkout", "HEAD", "--", "current.rs"]);
    fs::remove_file(repo.dir.path().join("candidate.rs")).unwrap();
    commit_at(&repo, &[], "delete candidate", "2025-02-12T00:00:00+0000");
    commit_at(
        &repo,
        &[("candidate.rs", "fn unrelated_recreated_file() {}\n")],
        "recreate candidate path",
        "2025-02-13T00:00:00+0000",
    );
    fs::write(repo.dir.path().join("current.rs"), format!("{alpha}{beta}")).unwrap();
    repo.index();

    let report = json(repo.run(["context", "--json", "--to-rev", &scoped_revision]));
    assert!(
        report["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|suggestion| suggestion["category"] != "historical_followup"),
        "out-of-scope delete/re-add boundaries must invalidate old file origins"
    );
}

fn complete_context_fixture(
    repo: &TestRepo,
    candidate_symlink: bool,
) -> (String, String, String, String) {
    commit_at(
        repo,
        &[
            ("current.rs", "fn placeholder() {}\n"),
            ("candidate.rs", "fn candidate_start() {}\n"),
            ("candidate-link-target.txt", "candidate-target.rs\n"),
            (
                "historical-a.rs",
                "fn alpha_feature() { let outcome = false; }\n",
            ),
            (
                "historical-b.rs",
                "fn beta_feature() { let outcome = false; }\n",
            ),
        ],
        "base",
        "2025-01-01T00:00:00+0000",
    );
    for day in 2..=11 {
        commit_at(
            repo,
            &[(
                &format!("decoy-{day}.rs"),
                &format!("fn unrelated_{day}() {{}}\n"),
            )],
            &format!("unrelated {day}"),
            &format!("2025-01-{day:02}T00:00:00+0000"),
        );
    }
    let alpha = "fn alpha_feature() { let outcome = identityAlpha(); }\n";
    let beta = "fn beta_feature() { let outcome = identityBeta(); }\n";
    commit_at(
        repo,
        &[("historical-a.rs", alpha)],
        "content origin alpha",
        "2025-02-01T00:00:00+0000",
    );
    let first_later = commit_at(
        repo,
        &[("candidate.rs", "fn candidate_first_followup() {}\n")],
        "first candidate follow-up",
        "2025-02-02T00:00:00+0000",
    );
    commit_at(
        repo,
        &[("historical-b.rs", beta)],
        "content origin beta",
        "2025-02-03T00:00:00+0000",
    );
    let second_later = commit_at(
        repo,
        &[("candidate.rs", "fn candidate_second_followup() {}\n")],
        "second candidate follow-up",
        "2025-02-04T00:00:00+0000",
    );
    if candidate_symlink {
        git(repo.dir.path(), ["config", "core.symlinks", "false"]);
        let link_blob = support::git_stdout(
            repo.dir.path(),
            ["rev-parse", "HEAD:candidate-link-target.txt"],
        );
        let cacheinfo = format!("120000,{link_blob},candidate.rs");
        git(repo.dir.path(), ["update-index", "--cacheinfo", &cacheinfo]);
        commit_index_at(
            repo,
            "candidate becomes symlink",
            "2025-02-05T00:00:00+0000",
        );
        git(repo.dir.path(), ["checkout", "HEAD", "--", "candidate.rs"]);
    }
    empty_commit_at(
        repo,
        "observation window complete",
        "2025-02-11T00:00:00+0000",
    );
    repo.index();
    fs::write(repo.dir.path().join("current.rs"), format!("{alpha}{beta}")).unwrap();
    (alpha.to_owned(), beta.to_owned(), first_later, second_later)
}

fn commit_index_at(repo: &TestRepo, message: &str, date: &str) -> String {
    let output = support::git_command(repo.dir.path())
        .args(["commit", "-m", message])
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    repo.head()
}

fn commit_at(repo: &TestRepo, files: &[(&str, &str)], message: &str, date: &str) -> String {
    for (path, content) in files {
        let path = repo.dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    git(repo.dir.path(), ["add", "--all"]);
    let output = support::git_command(repo.dir.path())
        .args(["commit", "-m", message])
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    repo.head()
}

fn empty_commit_at(repo: &TestRepo, message: &str, date: &str) -> String {
    let output = support::git_command(repo.dir.path())
        .args(["commit", "--allow-empty", "-m", message])
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    repo.head()
}

fn json(output: std::process::Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
