mod support;

use serde_json::Value;
use std::fs;
use support::TestRepo;

fn event(repo: &TestRepo, path: &str, value: usize, date: &str) {
    fs::write(repo.dir.path().join(path), format!("{value}\n")).unwrap();
    support::git(repo.dir.path(), ["add", "--all"]);
    let output = support::git_command(repo.dir.path())
        .args(["commit", "-m", path])
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fixture(repo: &TestRepo) {
    for (value, source, later) in [
        (0, "2025-01-01T12:00:00Z", "2025-01-02T12:00:00Z"),
        (1, "2025-01-20T12:00:00Z", "2025-01-21T12:00:00Z"),
    ] {
        event(repo, "source.rs", value, source);
        event(repo, "candidate.rs", value, later);
    }
    for day in 1..=8 {
        event(
            repo,
            "noise.txt",
            day,
            &format!("2025-02-{day:02}T12:00:00Z"),
        );
    }
    event(repo, "noise.txt", 99, "2025-02-20T12:00:00Z");
    repo.index();
    fs::write(
        repo.dir.path().join("source.rs"),
        "unique_unseen_content_214();\n",
    )
    .unwrap();
}

fn json(repo: &TestRepo, args: &[&str]) -> Value {
    let output = repo.run(args.iter().copied());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn context_discovers_path_follow_on_without_content_origins_using_related_policy() {
    let repo = TestRepo::new();
    fixture(&repo);
    let related = json(&repo, &["related", "source.rs", "--json"]);
    let report = json(&repo, &["context", "--json"]);
    let candidate = report["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["path"] == "candidate.rs")
        .expect("path-only candidate");
    assert_eq!(
        candidate["follow_on"],
        related["materials"][0]["detail"]["follow_on"]
    );
    assert_eq!(candidate["follow_on"][0]["supporting_origins"], 2);
    assert_eq!(candidate["follow_on"][0]["eligible_origins"], 2);
    assert_eq!(candidate["follow_on"][0]["independent_chains"], 2);
    let path_only = json(
        &repo,
        &[
            "context",
            "--json",
            "--no-historical-followup",
            "--max-followup-checks",
            "1",
            "--followup-days",
            "1",
        ],
    );
    assert_eq!(
        path_only["suggestions"][0]["follow_on"],
        candidate["follow_on"]
    );
    assert!(candidate["historical_followup"].is_null());
    assert!(candidate["content_matches"].as_array().unwrap().is_empty());
    let output = repo.run(["context"]);
    assert!(output.status.success());
    let human = String::from_utf8(output.stdout).unwrap();
    assert!(human.contains("source.rs -> candidate.rs"), "{human}");
    assert!(!human.contains("Supporting commits: 0"), "{human}");
    for example in candidate["follow_on"][0]["examples"].as_array().unwrap() {
        assert!(human.contains(example["origin_oid"].as_str().unwrap()));
        assert!(human.contains(example["later_oid"].as_str().unwrap()));
        assert_eq!(example["parent_distance"], 1);
        assert_eq!(example["elapsed_seconds"], 86400);
    }
    let clipped = json(&repo, &["context", "--json", "--until", "2025-01-22"]);
    assert!(
        clipped["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["follow_on"].as_array().unwrap().is_empty())
    );
}

#[test]
fn context_keeps_three_relationship_bases_and_individual_sources_in_one_slot() {
    let repo = TestRepo::new();
    let commit = |files: &[(&str, &str)], date: &str| {
        for (path, content) in files {
            fs::write(repo.dir.path().join(path), content).unwrap();
        }
        support::git(repo.dir.path(), ["add", "--all"]);
        let output = support::git_command(repo.dir.path())
            .args(["commit", "-m", "change"])
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    commit(
        &[
            ("source.rs", "placeholder\n"),
            ("companion.rs", "placeholder\n"),
            ("weak.rs", "placeholder\n"),
            ("candidate.rs", "placeholder\n"),
        ],
        "2024-12-01T12:00:00Z",
    );
    let alpha = "fn alpha_feature() { identityAlpha214(); }\n";
    let beta = "fn beta_feature() { identityBeta214(); }\n";
    commit(
        &[("source.rs", alpha), ("companion.rs", "first\n")],
        "2025-01-01T12:00:00Z",
    );
    event(&repo, "candidate.rs", 0, "2025-01-02T12:00:00Z");
    commit(
        &[("source.rs", beta), ("companion.rs", "second\n")],
        "2025-01-20T12:00:00Z",
    );
    event(&repo, "candidate.rs", 1, "2025-01-21T12:00:00Z");
    event(&repo, "weak.rs", 1, "2025-02-01T12:00:00Z");
    event(&repo, "candidate.rs", 2, "2025-02-02T12:00:00Z");
    for day in 1..=8 {
        event(
            &repo,
            "noise.txt",
            day,
            &format!("2025-03-{day:02}T12:00:00Z"),
        );
    }
    event(&repo, "noise.txt", 99, "2025-03-20T12:00:00Z");
    repo.index();
    fs::write(repo.dir.path().join("source.rs"), format!("{alpha}{beta}")).unwrap();
    fs::write(
        repo.dir.path().join("companion.rs"),
        "unseen_companion214();\n",
    )
    .unwrap();
    fs::write(repo.dir.path().join("weak.rs"), "unseen_weak214();\n").unwrap();
    let report = json(&repo, &["context", "--json"]);
    let candidates = report["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["path"] == "candidate.rs")
        .collect::<Vec<_>>();
    assert_eq!(candidates.len(), 1, "{report}");
    let candidate = candidates[0];
    assert_eq!(candidate["co_change"]["supporting_count"], 1, "{report}");
    assert_eq!(
        candidate["co_change"]["citations"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(candidate["historical_followup"]["supporting_origins"], 2);
    assert_eq!(candidate["historical_followup"]["complete_origins"], 2);
    assert_eq!(
        candidate["historical_followup"]["chains"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let sources = candidate["follow_on"].as_array().unwrap();
    assert_eq!(sources.len(), 2, "{report}");
    assert_eq!(sources[0]["source"], "companion.rs");
    assert_eq!(sources[1]["source"], "source.rs");
    for basis in sources {
        assert_eq!(basis["supporting_origins"], 2);
        assert_eq!(basis["eligible_origins"], 2);
        assert_eq!(basis["independent_chains"], 2);
        assert_eq!(basis["examples"].as_array().unwrap().len(), 2);
        let related = json(
            &repo,
            &["related", basis["source"].as_str().unwrap(), "--json"],
        );
        let related_candidate = related["materials"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["paths"][0] == "candidate.rs")
            .unwrap();
        assert_eq!(*basis, related_candidate["detail"]["follow_on"][0]);
    }
    let limited = json(&repo, &["context", "--limit", "1", "--json"]);
    assert_eq!(limited["matched_count"], report["matched_count"]);
    assert_eq!(limited["suggestions"].as_array().unwrap().len(), 1);
    assert_eq!(limited["truncated"], true);
}
