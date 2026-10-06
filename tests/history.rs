mod support;

use std::{fs, path::Path};

use support::{TestRepo, git, git_command, git_stdout};

impl TestRepo {
    /// Commit with an explicit author and committer date, so ordering never depends on
    /// how fast the test runs.
    fn commit_at(&self, path: &str, contents: &[u8], message: &str, date: &str) -> String {
        if let Some(parent) = Path::new(path).parent() {
            fs::create_dir_all(self.dir.path().join(parent)).expect("create parent directory");
        }
        fs::write(self.dir.path().join(path), contents).expect("write tracked file");
        git(self.dir.path(), ["add", "--all"]);
        let output = git_command(self.dir.path())
            .args(["commit", "-m", message])
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        self.head()
    }

    fn commit_files_at(&self, files: &[(&str, &[u8])], message: &str, date: &str) -> String {
        for (path, contents) in files {
            if let Some(parent) = Path::new(path).parent() {
                fs::create_dir_all(self.dir.path().join(parent)).expect("create parent directory");
            }
            fs::write(self.dir.path().join(path), contents).expect("write tracked file");
        }
        git(self.dir.path(), ["add", "--all"]);
        let output = git_command(self.dir.path())
            .args(["commit", "-m", message])
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        self.head()
    }

    /// Remove a path and record the removal, so it exists only in history afterwards.
    fn remove(&self, path: &str, message: &str) -> String {
        fs::remove_file(self.dir.path().join(path)).expect("remove tracked file");
        git(self.dir.path(), ["add", "--all"]);
        git(self.dir.path(), ["commit", "-m", message]);
        self.head()
    }
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The block of one result, up to the next result marker.
fn block<'a>(text: &'a str, oid_prefix: &str) -> &'a str {
    let start = text
        .find(oid_prefix)
        .unwrap_or_else(|| panic!("result {oid_prefix} missing from:\n{text}"));
    let rest = &text[start..];
    match rest[1..].find("\n- ") {
        Some(offset) => &rest[..offset + 1],
        None => rest,
    }
}

/// Two analogous provider retirements, the shape `examples` exists to surface.
fn provider_retirements() -> TestRepo {
    let repo = TestRepo::new();
    repo.commit_at(
        "src/providers/tavily.rs",
        b"tavily backend\n",
        "Retire TavilyProvider across registration, aliases and tests",
        "2020-01-01T00:00:00+0000",
    );
    repo.commit_at(
        "src/providers/mod.rs",
        b"registry: tavily\n",
        "Register Tavily provider aliases",
        "2020-01-02T00:00:00+0000",
    );
    repo
}

#[test]
fn examples_returns_analogous_changes_with_reusable_steps() {
    let repo = provider_retirements();
    repo.commit_at(
        "src/providers/brave.rs",
        b"brave backend\n",
        "Retire BraveProvider while preserving migration guidance",
        "2021-01-01T00:00:00+0000",
    );
    repo.index();

    let output = repo.run(["examples", "retire", "provider"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("Retire BraveProvider"), "{text}");
    assert!(text.contains("Retire TavilyProvider"), "{text}");
    assert!(text.contains("step:"), "{text}");
    assert!(text.contains("confidence:"), "{text}");
    assert!(text.contains("basis:"), "{text}");

    // Steps describe what history did; they must not instruct the caller.
    for mandate in [" must ", " should ", "you need to", "required to"] {
        assert!(
            !text.contains(mandate),
            "steps read as a mandate via {mandate:?}:\n{text}"
        );
    }
}

#[test]
fn examples_accepts_an_anchored_path_and_prefers_overlapping_change() {
    let repo = provider_retirements();
    repo.commit_at(
        "src/providers/brave.rs",
        b"brave backend\n",
        "Retire BraveProvider while preserving migration guidance",
        "2021-01-01T00:00:00+0000",
    );
    repo.commit_at(
        "docs/providers.md",
        b"provider docs\n",
        "Retire provider references from the documentation",
        "2021-02-01T00:00:00+0000",
    );
    repo.index();

    let anchored = repo.run([
        "examples",
        "retire",
        "provider",
        "--path",
        "src/providers/brave.rs",
    ]);
    assert_eq!(anchored.status.code(), Some(0), "{}", stderr(&anchored));
    let text = stdout(&anchored);
    assert!(text.contains("exact path match"), "{text}");
    assert!(text.contains("anchored path match"), "{text}");

    // The anchored query must put the overlapping change ahead of the documentation change.
    let brave = text.find("Retire BraveProvider").expect("brave result");
    let docs = text.find("documentation").unwrap_or(usize::MAX);
    assert!(brave < docs, "anchored result not preferred:\n{text}");
}

#[test]
fn examples_excludes_candidates_from_unmatched_paths() {
    let repo = TestRepo::new();
    let matching = repo.commit_at(
        "src/a.rs",
        b"provider implementation\n",
        "Retire provider implementation",
        "2020-01-01T00:00:00+0000",
    );
    let unrelated = repo.commit_at(
        "docs/providers.md",
        b"provider documentation\n",
        "Retire provider documentation",
        "2020-02-01T00:00:00+0000",
    );
    repo.index();

    let output = repo.run(["examples", "retire", "provider", "--path", "./SRC/a.rs"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("Historical examples (1 match):"), "{text}");
    assert!(text.contains(&matching[..12]), "{text}");
    assert!(
        !text
            .lines()
            .skip(1)
            .any(|line| line.contains(&unrelated[..12])),
        "candidate from an unmatched path was returned:\n{text}"
    );
}

#[test]
fn examples_filters_paths_before_candidate_limit() {
    let repo = TestRepo::new();
    let matching = repo.commit_at(
        "src/very/long/neutral/path/with/many/components/that/do/not/match/the/query/words/target.rs",
        b"implementation\n",
        "Retire provider implementation",
        "2020-01-01T00:00:00+0000",
    );
    for day in 2..=22 {
        repo.commit_at(
            "z",
            format!("documentation {day}\n").as_bytes(),
            "Retire provider implementation",
            &format!("2020-01-{day:02}T00:00:00+0000"),
        );
    }
    repo.index();

    let output = repo.run([
        "examples",
        "retire",
        "provider",
        "--path",
        "src/very/long/neutral/path/with/many/components/that/do/not/match/the/query/words/target.rs",
        "--limit",
        "1",
    ]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("Historical examples (1 match):"), "{text}");
    assert!(
        text.contains(&matching[..12]),
        "path match fell outside lexical pool:\n{text}"
    );
}

#[test]
fn examples_filter_scoped_results_by_path_basename() {
    let repo = TestRepo::new();
    let matching = repo.commit_at(
        "src/a.rs",
        b"provider implementation\n",
        "Retire provider implementation",
        "2020-01-01T00:00:00+0000",
    );
    let unrelated = repo.commit_at(
        "docs/providers.md",
        b"provider documentation\n",
        "Retire provider documentation",
        "2020-01-02T00:00:00+0000",
    );
    repo.index();

    let output = repo.run([
        "examples",
        "retire",
        "provider",
        "--path",
        "a.rs",
        "--since",
        "2020-01-01",
        "--until",
        "2020-01-31",
    ]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("Historical examples (1 match):"), "{text}");
    assert!(text.contains(&matching[..12]), "{text}");
    assert!(
        !text.contains(&format!("\n- {} ", &unrelated[..12])),
        "scoped candidate from an unmatched basename was returned:\n{text}"
    );
}

#[test]
fn path_like_query_words_remain_soft_anchors() {
    let repo = TestRepo::new();
    let matching = repo.commit_at(
        "src/a.rs",
        b"provider implementation\n",
        "Retire provider implementation",
        "2020-01-01T00:00:00+0000",
    );
    let unrelated = repo.commit_at(
        "docs/providers.md",
        b"provider documentation\n",
        "Retire provider documentation",
        "2020-01-02T00:00:00+0000",
    );
    repo.index();

    let output = repo.run(["examples", "retire", "src/a.rs"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains(&matching[..12]), "{text}");
    assert!(
        text.contains(&unrelated[..12]),
        "path-like query anchor incorrectly filtered results:\n{text}"
    );
}

#[test]
fn examples_demotes_reverted_work_and_cites_the_revert() {
    let repo = TestRepo::new();
    // Two changes answer the query identically, so only the revert can order them.
    let kept = repo.commit_at(
        "src/providers/registry.rs",
        b"registry v1
",
        "Retire TavilyProvider registration",
        "2021-01-01T00:00:00+0000",
    );
    let abandoned = repo.commit_at(
        "src/providers/registry.rs",
        b"registry v2
",
        "Retire TavilyProvider registration",
        "2021-02-01T00:00:00+0000",
    );
    repo.commit_at(
        "src/providers/registry.rs",
        b"registry v3
",
        &format!(
            "revert: retire TavilyProvider registration

             This reverts commit {abandoned}.
"
        ),
        "2021-03-01T00:00:00+0000",
    );
    repo.index();

    let output = repo.run(["examples", "retire", "tavily", "provider", "registration"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);

    let abandoned_block = block(&text, &abandoned[..12]);
    assert!(
        abandoned_block.contains("demoted: later reverted"),
        "{abandoned_block}"
    );
    assert!(
        abandoned_block.contains("confidence: low"),
        "{abandoned_block}"
    );
    assert!(
        abandoned_block.contains("(later reverted)"),
        "{abandoned_block}"
    );

    // Abandoned work is not precedent, so the surviving change outranks it.
    let kept_position = text.find(&kept[..12]).expect("kept result");
    let abandoned_position = text.find(&abandoned[..12]).expect("abandoned result");
    assert!(
        kept_position < abandoned_position,
        "reverted work was not demoted:
{text}"
    );
}

#[test]
fn failures_choose_the_earliest_revert_by_history_order_not_timestamp() {
    let repo = TestRepo::new();
    let abandoned = repo.commit_at(
        "src/db/index.rs",
        b"cron sessions excluded\n",
        "Exclude cron sessions from the FTS index",
        "2026-01-01T00:00:00+0000",
    );
    let earliest_revert = repo.commit_at(
        "src/db/index.rs",
        b"cron sessions restored\n",
        &format!(
            "revert: exclude cron sessions from the FTS index\n\n\
             This reverts commit {abandoned}.\n\n\
             The earlier approach failed because it blocked the scheduled writer.\n"
        ),
        "2026-03-01T00:00:00+0000",
    );
    let later_revert = repo.commit_at(
        "src/db/index.rs",
        b"cron sessions reconfigured\n",
        &format!(
            "revert: exclude cron sessions from the FTS index\n\n\
             This reverts commit {abandoned}.\n\n\
             The later revert claims the flag broke lookups.\n"
        ),
        "2026-02-01T00:00:00+0000",
    );
    repo.index();

    let outputs = [
        (
            "unscoped",
            repo.run(["failures", "cron", "sessions", "--json"]),
        ),
        (
            "scoped",
            repo.run([
                "failures",
                "cron",
                "sessions",
                "--to-rev",
                later_revert.as_str(),
                "--json",
            ]),
        ),
    ];
    let observations = outputs
        .into_iter()
        .map(|(scope, output)| {
            assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
            let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            let entry = report["materials"]
                .as_array()
                .unwrap()
                .iter()
                .find(|material| material["subject"] == "Exclude cron sessions from the FTS index")
                .expect("abandoned change in failures report");
            let reason = entry["detail"]["reason"]
                .as_str()
                .unwrap_or("<missing reason>")
                .to_owned();
            let revert_oid = entry["citations"]
                .as_array()
                .unwrap()
                .iter()
                .find(|citation| citation["note"] == "reverts this change")
                .and_then(|citation| citation["oid"].as_str())
                .unwrap_or("<missing revert citation>")
                .to_owned();
            (scope, reason, revert_oid)
        })
        .collect::<Vec<_>>();
    assert!(
        observations.iter().all(|(_, reason, oid)| {
            reason == "The earlier approach failed because it blocked the scheduled writer"
                && oid == &earliest_revert
                && oid != &later_revert
        }),
        "unexpected failure links: {observations:#?}"
    );
}

#[test]
fn failures_reports_the_stated_reason_and_the_corrective_follow_up() {
    let repo = TestRepo::new();
    let abandoned = repo.commit_at(
        "src/db/index.rs",
        b"cron sessions\n",
        "Exclude cron sessions from the FTS index",
        "2020-01-01T00:00:00+0000",
    );
    repo.commit_at(
        "src/db/index.rs",
        b"cron sessions restored\n",
        &format!(
            "revert: exclude cron sessions from the FTS index\n\n\
             This reverts commit {abandoned}.\n\n\
             The shared predicate slowed the hot ingestion path for every provider.\n\n\
             Re-land criteria: gate the exclusion to the cron writer and measure ingestion latency.\n"
        ),
        "2020-02-01T00:00:00+0000",
    );
    let revert = repo.head();
    // Touches the same path first, but corrects nothing, so it must not be claimed.
    repo.commit_at(
        "src/db/index.rs",
        b"cron sessions (reformatted)
",
        "chore(db): reformat the index module",
        "2020-02-15T00:00:00+0000",
    );
    let follow_up = repo.commit_at(
        "src/db/index.rs",
        b"cron sessions (gated)\n",
        "fix(db): gate the cron session exclusion to its own writer",
        "2020-03-01T00:00:00+0000",
    );
    repo.index();

    let output = repo.run(["failures", "exclude", "cron", "sessions"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    let entry = block(&text, &abandoned[..12]);

    assert!(
        entry.contains("reason: The shared predicate slowed the hot ingestion path"),
        "{entry}"
    );
    assert!(
        entry.contains("retry: gate the exclusion to the cron writer"),
        "{entry}"
    );
    assert!(entry.contains(&revert[..12]), "{entry}");
    assert!(entry.contains("(reverts this change)"), "{entry}");
    assert!(entry.contains(&follow_up[..12]), "{entry}");
    assert!(entry.contains("(follow-up)"), "{entry}");
    assert!(entry.contains("basis:"), "{entry}");

    // The revert is told by the change it undid, so it is not also its own result.
    assert!(
        !text.contains(&format!("\n- {} ", &revert[..12])),
        "revert repeated as its own result:\n{text}"
    );
}

#[test]
fn failures_does_not_link_parallel_branch_corrective_commit() {
    let repo = TestRepo::new();
    let base = repo.commit_at("src/x.rs", b"base\n", "base", "2001-01-01T00:00:00+0000");
    git(repo.dir.path(), ["switch", "-c", "candidate"]);
    let candidate = repo.commit_at(
        "src/x.rs",
        b"candidate approach\n",
        "Try candidate approach",
        "2001-01-02T00:00:00+0000",
    );
    let revert_output = git_command(repo.dir.path())
        .args(["revert", "--no-edit", candidate.as_str()])
        .env("GIT_AUTHOR_DATE", "2001-01-03T00:00:00+0000")
        .env("GIT_COMMITTER_DATE", "2001-01-03T00:00:00+0000")
        .output()
        .expect("revert candidate");
    assert!(revert_output.status.success(), "{}", stderr(&revert_output));
    let revert = repo.head();

    git(
        repo.dir.path(),
        ["switch", "-c", "parallel-fix", base.as_str()],
    );
    let follow_up = repo.commit_at(
        "src/x.rs",
        b"candidate approach fixed\n",
        "fix: candidate approach",
        "2001-01-04T00:00:00+0000",
    );
    let is_descendant = git_command(repo.dir.path())
        .args([
            "merge-base",
            "--is-ancestor",
            revert.as_str(),
            follow_up.as_str(),
        ])
        .status()
        .expect("check follow-up ancestry")
        .success();
    assert!(
        !is_descendant,
        "fixture follow-up must be on a parallel branch"
    );

    git(repo.dir.path(), ["switch", "main"]);
    git(repo.dir.path(), ["merge", "--ff-only", "candidate"]);
    git(
        repo.dir.path(),
        [
            "merge",
            "--no-ff",
            "-s",
            "ours",
            "parallel-fix",
            "-m",
            "merge parallel fix",
        ],
    );
    repo.index();

    let cache =
        rusqlite::Connection::open(repo.cache_dir().join("cache.sqlite")).expect("open test cache");
    let position = |oid: &str| {
        cache
            .query_row(
                "SELECT position FROM commits WHERE oid = ?1",
                [oid],
                |row| row.get::<_, i64>(0),
            )
            .expect("read cache position")
    };
    let revert_position = position(&revert);
    let follow_up_position = position(&follow_up);
    assert!(
        revert_position < follow_up_position,
        "fixture requires later-position parallel follow-up: revert={revert_position}, follow-up={follow_up_position}"
    );

    let output = repo.run(["failures", "candidate", "approach"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    let entry = block(&text, &candidate[..12]);
    assert!(entry.contains(&revert[..12]), "{entry}");
    assert!(
        !entry.contains(&follow_up[..12]) && !entry.contains("(follow-up)"),
        "parallel-branch corrective commit incorrectly linked:\n{entry}"
    );
}

#[test]
fn failures_excludes_candidates_from_unmatched_paths() {
    let repo = TestRepo::new();
    let matching = repo.commit_at(
        "src/a.rs",
        b"cron sessions excluded\n",
        "Exclude cron sessions from the FTS index",
        "2020-01-01T00:00:00+0000",
    );
    repo.commit_at(
        "src/a.rs",
        b"cron sessions restored\n",
        &format!(
            "revert: exclude cron sessions from the FTS index\n\n\
             This reverts commit {matching}.\n\n\
             The shared predicate slowed ingestion.\n\n\
             Re-land criteria: gate the exclusion to the cron writer.\n"
        ),
        "2020-02-01T00:00:00+0000",
    );
    let unrelated = repo.commit_at(
        "docs/cron.md",
        b"cron sessions excluded from docs\n",
        "Exclude cron sessions from provider documentation",
        "2020-03-01T00:00:00+0000",
    );
    repo.commit_at(
        "docs/cron.md",
        b"cron sessions restored in docs\n",
        &format!(
            "revert: exclude cron sessions from provider documentation\n\n\
             This reverts commit {unrelated}.\n\n\
             The documentation change confused users.\n\n\
             Re-land criteria: clarify only the affected provider pages.\n"
        ),
        "2020-04-01T00:00:00+0000",
    );
    repo.index();

    let output = repo.run([
        "failures", "exclude", "cron", "sessions", "--path", "src/a.rs",
    ]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("Failed approaches (1 match):"), "{text}");
    assert!(text.contains(&matching[..12]), "{text}");
    assert!(
        !text
            .lines()
            .skip(1)
            .any(|line| line.contains(&unrelated[..12])),
        "candidate from an unmatched path was returned:\n{text}"
    );
}

#[test]
fn failures_handle_more_than_sql_parameter_limit_paths() {
    let repo = TestRepo::new();
    let abandoned_contents = (0..901)
        .map(|index| {
            (
                format!("src/generated/module-{index}.rs"),
                format!("generated module {index}\n").into_bytes(),
            )
        })
        .collect::<Vec<_>>();
    let abandoned_files = abandoned_contents
        .iter()
        .map(|(path, contents)| (path.as_str(), contents.as_slice()))
        .collect::<Vec<_>>();
    let abandoned = repo.commit_files_at(
        &abandoned_files,
        "Exclude generated modules from the build",
        "2020-01-01T00:00:00+0000",
    );
    let restored_contents = abandoned_contents
        .iter()
        .map(|(path, _)| (path.clone(), b"restored module\n".to_vec()))
        .collect::<Vec<_>>();
    let restored_files = restored_contents
        .iter()
        .map(|(path, contents)| (path.as_str(), contents.as_slice()))
        .collect::<Vec<_>>();
    repo.commit_files_at(
        &restored_files,
        &format!(
            "revert: exclude generated modules from the build\n\n\
             This reverts commit {abandoned}.\n\n\
             The generated module set broke startup.\n\n\
             Re-land criteria: generate only requested modules.\n"
        ),
        "2020-02-01T00:00:00+0000",
    );
    repo.commit_at(
        &abandoned_contents[0].0,
        b"generated module fixed\n",
        "fix(build): generate only requested modules",
        "2020-03-01T00:00:00+0000",
    );
    repo.index();

    let output = repo.run(["failures", "generated", "modules"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    let entry = block(&text, &abandoned[..12]);
    assert!(
        entry.contains("reason: The generated module set broke startup"),
        "{entry}"
    );
    assert!(entry.contains("(follow-up)"), "{entry}");
}

#[test]
fn failures_prints_reason_unknown_rather_than_inventing_one() {
    let repo = TestRepo::new();
    let abandoned = repo.commit_at(
        "src/db/index.rs",
        b"cron sessions\n",
        "Exclude cron sessions from the FTS index",
        "2020-01-01T00:00:00+0000",
    );
    // A revert that records no reason at all beyond the trailer.
    repo.commit_at(
        "src/db/index.rs",
        b"cron sessions restored\n",
        &format!(
            "revert: exclude cron sessions from the FTS index\n\n\
             This reverts commit {abandoned}.\n"
        ),
        "2020-02-01T00:00:00+0000",
    );
    repo.index();

    let output = repo.run(["failures", "exclude", "cron", "sessions"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    let entry = block(&text, &abandoned[..12]);

    assert!(entry.contains("reason: Reason unknown"), "{entry}");
    assert!(
        !entry.contains("retry:"),
        "retry condition invented without history:\n{entry}"
    );
}

#[test]
fn examples_and_failures_accept_a_path_that_exists_only_in_history() {
    let repo = TestRepo::new();
    let original = repo.commit_at(
        "src/db/legacy_index.rs",
        b"legacy cron exclusion\n",
        "Exclude cron sessions from the FTS index",
        "2020-01-01T00:00:00+0000",
    );
    repo.commit_at(
        "src/db/legacy_index.rs",
        b"legacy cron exclusion removed\n",
        &format!(
            "revert: exclude cron sessions from the FTS index\n\n\
             This reverts commit {original}.\n\n\
             The shared predicate slowed ingestion for every caller.\n"
        ),
        "2020-02-01T00:00:00+0000",
    );
    repo.remove(
        "src/db/legacy_index.rs",
        "refactor(db): drop the unused legacy index module",
    );
    assert!(!repo.dir.path().join("src/db/legacy_index.rs").exists());
    repo.index();

    let examples = repo.run([
        "examples",
        "cron",
        "exclusions",
        "--path",
        "src/db/legacy_index.rs",
    ]);
    assert_eq!(examples.status.code(), Some(0), "{}", stderr(&examples));
    assert!(
        stdout(&examples).contains(&original[..12]),
        "{}",
        stdout(&examples)
    );

    let failures = repo.run([
        "failures",
        "cron",
        "exclusions",
        "--path",
        "src/db/legacy_index.rs",
    ]);
    assert_eq!(failures.status.code(), Some(0), "{}", stderr(&failures));
    let text = stdout(&failures);
    assert!(text.contains(&original[..12]), "{text}");
    assert!(
        text.contains("reason: The shared predicate slowed ingestion"),
        "{text}"
    );
}

#[test]
fn examples_and_failures_report_their_fixed_empty_results() {
    let repo = provider_retirements();

    repo.index();
    let examples = repo.run(["examples", "term-that-does-not-exist"]);
    assert_eq!(examples.status.code(), Some(0));
    assert!(stdout(&examples).ends_with("No historical examples found.\n"));

    let failures = repo.run(["failures", "term-that-does-not-exist"]);
    assert_eq!(failures.status.code(), Some(0));
    assert!(stdout(&failures).ends_with("No failed approaches found.\n"));
}

#[test]
fn failures_report_counts_only_eligible_results_and_truncates_against_them() {
    let repo = TestRepo::new();
    let widget = repo.commit_at(
        "src/widget.rs",
        b"widget strategy enabled\n",
        "Try widget strategy",
        "2020-01-01T00:00:00+0000",
    );
    repo.commit_at(
        "src/widget.rs",
        b"widget strategy reverted\n",
        &format!("revert: try widget strategy\n\nThis reverts commit {widget}.\n"),
        "2020-01-02T00:00:00+0000",
    );
    repo.commit_at(
        "docs/widget.md",
        b"Widget compatibility notes\n",
        "Document widget compatibility",
        "2020-01-03T00:00:00+0000",
    );

    let sandbox_one = repo.commit_at(
        "src/sandbox/one.rs",
        b"sandbox approach one\n",
        "Try sandbox approach one",
        "2020-01-04T00:00:00+0000",
    );
    repo.commit_at(
        "src/sandbox/one.rs",
        b"sandbox approach one reverted\n",
        &format!("revert: try sandbox approach one\n\nThis reverts commit {sandbox_one}.\n"),
        "2020-01-05T00:00:00+0000",
    );
    let sandbox_two = repo.commit_at(
        "src/sandbox/two.rs",
        b"sandbox approach two\n",
        "Try sandbox approach two",
        "2020-01-06T00:00:00+0000",
    );
    repo.commit_at(
        "src/sandbox/two.rs",
        b"sandbox approach two reverted\n",
        &format!("revert: try sandbox approach two\n\nThis reverts commit {sandbox_two}.\n"),
        "2020-01-07T00:00:00+0000",
    );
    repo.commit_at(
        "docs/sandbox.md",
        b"Sandbox compatibility notes\n",
        "Document sandbox compatibility",
        "2020-01-08T00:00:00+0000",
    );
    repo.commit_at(
        "docs/ghost-one.md",
        b"Ghost compatibility notes\n",
        "Document ghost compatibility one",
        "2020-01-09T00:00:00+0000",
    );
    repo.commit_at(
        "docs/ghost-two.md",
        b"Ghost compatibility notes\n",
        "Document ghost compatibility two",
        "2020-01-10T00:00:00+0000",
    );
    repo.index();

    for (query, limit, matched_count, truncated, material_count) in [
        ("ghost", "1", 0, false, 0),
        ("widget", "100", 1, false, 1),
        ("sandbox", "1", 2, true, 1),
    ] {
        let output = repo.run(["failures", query, "--limit", limit, "--json"]);
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();

        assert_eq!(report["matched_count"], matched_count, "{query}: {report}");
        assert_eq!(report["truncated"], truncated, "{query}: {report}");
        assert_eq!(
            report["materials"].as_array().unwrap().len(),
            material_count,
            "{query}: {report}"
        );
    }
}

#[test]
fn examples_truncates_deterministically_and_rejects_invalid_input() {
    let repo = provider_retirements();
    for (index, date) in ["2021-01-01", "2021-02-01", "2021-03-01"]
        .iter()
        .enumerate()
    {
        repo.commit_at(
            &format!("src/providers/extra{index}.rs"),
            format!("extra provider {index}\n").as_bytes(),
            "Retire ExtraProvider across registration surfaces",
            &format!("{date}T00:00:00+0000"),
        );
    }
    repo.index();

    let first = repo.run(["examples", "retire", "provider", "--limit", "2"]);
    assert_eq!(first.status.code(), Some(0), "{}", stderr(&first));
    let text = stdout(&first);
    assert_eq!(text.matches("confidence:").count(), 2, "{text}");
    assert!(text.contains("results truncated"), "{text}");
    assert!(text.contains("Showing 2 of 5"), "{text}");

    let again = repo.run(["examples", "retire", "provider", "--limit", "2"]);
    assert_eq!(stdout(&again), text);

    for invalid in [
        vec!["examples"],
        vec!["examples", "provider", "--limit", "0"],
        vec!["examples", "provider", "--path", ""],
        vec!["failures"],
        vec!["failures", "provider", "--limit", "0"],
        vec!["failures", "provider", "--path", ""],
    ] {
        let output = repo.run(invalid.clone());
        assert_eq!(
            output.status.code(),
            Some(2),
            "{invalid:?} should be invalid input: {}{}",
            stdout(&output),
            stderr(&output)
        );
    }
}

#[test]
fn examples_and_failures_reuse_the_cache_without_preparation_noise() {
    let repo = provider_retirements();
    repo.commit_at(
        "src/providers/brave.rs",
        b"brave backend\n",
        "Retire BraveProvider while preserving migration guidance",
        "2021-01-01T00:00:00+0000",
    );

    repo.index();
    let first = repo.run(["examples", "retire", "provider"]);
    assert_eq!(first.status.code(), Some(0));
    assert!(stderr(&first).is_empty(), "{}", stderr(&first));

    let second = repo.run(["examples", "retire", "provider"]);
    assert_eq!(second.status.code(), Some(0));
    assert!(
        stderr(&second).is_empty(),
        "fresh query was noisy: {}",
        stderr(&second)
    );
    assert_eq!(stdout(&second), stdout(&first));

    // The same completed cache answers the failure capability too.
    let failures = repo.run(["failures", "retire", "provider"]);
    assert_eq!(failures.status.code(), Some(0), "{}", stderr(&failures));
    assert!(stderr(&failures).is_empty(), "{}", stderr(&failures));
}

#[test]
fn missing_history_objects_fail_with_the_git_diagnosis_and_publish_nothing() {
    let repo = TestRepo::new();
    repo.commit_at(
        "src/a.txt",
        b"a
",
        "Initial commit",
        "2020-01-01T00:00:00+0000",
    );

    // Remove a tree Git must read to derive changes. The failure surfaces as a broken
    // pipe while GitScript is still feeding `diff-tree`, which must not mask Git's own
    // diagnosis of what is missing.
    let tree = git_stdout(repo.dir.path(), ["rev-parse", "HEAD^{tree}"]);
    let object = repo
        .dir
        .path()
        .join(".git/objects")
        .join(&tree[..2])
        .join(&tree[2..]);
    fs::remove_file(object).expect("remove tree object");

    let output = repo.run(["index"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let message = stderr(&output);
    assert!(message.contains("unable to read tree"), "{message}");
    assert!(
        !message.contains("sending input to Git"),
        "Git's diagnosis was masked: {message}"
    );
    assert!(!repo.cache_dir().join("cache.sqlite").exists());
}

#[test]
fn failures_ignores_commits_that_merely_mention_a_revert() {
    let repo = TestRepo::new();
    // Mentions a revert in its body but is not abandoned work, so it is not a failed
    // approach however much its text reads like a bug report.
    let mentions = repo.commit_at(
        "src/db/index.rs",
        b"cron exclusion restored\n",
        "fix(db): keep the cron exclusion alive across rotations

The earlier attempt was reverted in #1234 because the predicate was shared, so
this one gates the exclusion to the single writer that needs it.",
        "2020-01-01T00:00:00+0000",
    );
    let abandoned = repo.commit_at(
        "src/db/index.rs",
        b"cron exclusion\n",
        "Exclude cron sessions from the FTS index",
        "2020-02-01T00:00:00+0000",
    );
    repo.commit_at(
        "src/db/index.rs",
        b"cron exclusion undone\n",
        &format!(
            "revert: exclude cron sessions from the FTS index\n\n\
             This reverts commit {abandoned}.\n\n\
             The shared predicate slowed ingestion for every caller.\n"
        ),
        "2020-03-01T00:00:00+0000",
    );
    repo.index();

    let output = repo.run(["failures", "cron", "exclusion"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);

    assert!(text.contains(&abandoned[..12]), "{text}");
    assert!(
        !text.contains(&mentions[..12]),
        "a commit that only mentions a revert was reported as a failed approach:\n{text}"
    );
    assert!(
        text.contains("reason: The shared predicate slowed ingestion"),
        "{text}"
    );
}

#[test]
fn failures_reads_a_reason_and_retry_written_across_wrapped_lines() {
    let repo = TestRepo::new();
    let abandoned = repo.commit_at(
        "src/db/index.rs",
        b"cron sessions\n",
        "Exclude cron sessions from the FTS index",
        "2020-01-01T00:00:00+0000",
    );
    // Commit bodies are hard-wrapped, so a stated reason and retry condition both span
    // several lines; neither may be cut at the line break.
    repo.commit_at(
        "src/db/index.rs",
        b"cron sessions restored\n",
        &format!(
            "revert: exclude cron sessions from the FTS index\n\n\
             This reverts commit {abandoned}.\n\n\
             The shared predicate slowed the hot ingestion\n\
             path for every provider.\n\n\
             The narrow bug should be re-addressed provider-gated\n\
             rather than on the shared path.\n"
        ),
        "2020-02-01T00:00:00+0000",
    );
    repo.index();

    let output = repo.run(["failures", "exclude", "cron", "sessions"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    let entry = block(&text, &abandoned[..12]);

    assert!(
        entry.contains(
            "reason: The shared predicate slowed the hot ingestion path for every provider"
        ),
        "{entry}"
    );
    assert!(
        entry.contains("retry: provider-gated rather than on the shared path"),
        "{entry}"
    );
}

#[test]
fn examples_bounds_the_steps_it_offers() {
    let repo = TestRepo::new();
    // One change touching many files: the reusable pattern must not be buried in a
    // listing of every path it moved.
    for index in 0..12 {
        fs::create_dir_all(repo.dir.path().join("src/providers")).expect("create directory");
        fs::write(
            repo.dir
                .path()
                .join(format!("src/providers/bulk{index}.rs")),
            format!(
                "provider {index}
"
            ),
        )
        .expect("write tracked file");
    }
    git(repo.dir.path(), ["add", "--all"]);
    git(
        repo.dir.path(),
        [
            "commit",
            "-m",
            "Retire BulkProvider across registration and aliases",
        ],
    );
    repo.index();

    let output = repo.run(["examples", "retire", "bulk", "provider", "registration"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);

    assert!(text.contains("more steps"), "{text}");
    let shown = text.matches("  step: ").count();
    assert!(
        shown <= 8,
        "too many steps shown ({shown}):
{text}"
    );
}

#[test]
fn failures_never_takes_a_reason_from_the_abandoned_change() {
    let repo = TestRepo::new();
    // The abandoned change states a reason of its own; the revert states none. Only the
    // revert can confirm why the work was undone, so the honest answer is Reason unknown.
    let abandoned = repo.commit_at(
        "src/db/index.rs",
        b"cron sessions\n",
        "Exclude cron sessions from the FTS index

Cron sessions are noisy in search results, so we filter them here.",
        "2020-01-01T00:00:00+0000",
    );
    repo.commit_at(
        "src/db/index.rs",
        b"cron sessions restored\n",
        &format!(
            "revert: exclude cron sessions from the FTS index\n\n\
             This reverts commit {abandoned}.\n"
        ),
        "2020-02-01T00:00:00+0000",
    );
    repo.index();

    let output = repo.run(["failures", "exclude", "cron", "sessions"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    let entry = block(&text, &abandoned[..12]);

    assert!(entry.contains("reason: Reason unknown"), "{entry}");
    assert!(
        !entry.contains("noisy"),
        "a reason was inferred from the abandoned change:\n{entry}"
    );
    // No recorded reason and no correction, so the lexical match is all the support there is.
    assert!(!entry.contains("confidence: high"), "{entry}");
}

#[test]
fn failures_do_not_link_a_revert_whose_named_commit_is_absent() {
    let repo = TestRepo::new();
    let abandoned = repo.commit_at(
        "src/db/index.rs",
        b"cron sessions
",
        "Exclude cron sessions from the FTS index",
        "2020-01-01T00:00:00+0000",
    );
    let revert = repo.commit_at(
        "src/db/index.rs",
        b"cron sessions restored
",
        "revert: exclude cron sessions from the FTS index

         This reverts commit 0123456789abcdef0123456789abcdef01234567.

         The predicate was too broad for the shared writer.
",
        "2020-02-01T00:00:00+0000",
    );
    repo.index();

    let output = repo.run(["failures", "exclude", "cron", "sessions"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    // The declaration names an unknown commit, so it binds to nothing; the reverted
    // paths alone must never attribute the revert to the abandoned change.
    assert!(
        !text.contains(&abandoned[..12]),
        "an explicit revert naming an unknown commit was linked by shared paths:\n{text}"
    );
    assert!(
        !text.contains(&format!(
            "
- {} ",
            &revert[..12]
        )),
        "an unresolved revert was reported as its own failed approach:\n{text}"
    );
}

#[test]
fn failures_subject_only_revert_never_links_without_a_declaration() {
    // Shared paths plus a revert-like subject identify nothing; only an explicit
    // declaration names a target, so the abandoned change is reported without one.
    let valid = TestRepo::new();
    let abandoned = valid.commit_files_at(
        &[
            ("src/db/index.rs", b"cron sessions\n"),
            ("src/db/query.rs", b"cron sessions query\n"),
        ],
        "Exclude cron sessions from the FTS index",
        "2020-01-01T00:00:00+0000",
    );
    valid.commit_files_at(
        &[
            ("src/db/index.rs", b"cron sessions restored\n"),
            ("src/db/query.rs", b"cron sessions query restored\n"),
        ],
        "Revert the old database approach",
        "2020-02-01T00:00:00+0000",
    );
    valid.index();
    // Shared paths alone never attribute the revert; neither the abandoned change nor
    // the subject-only revert is reported as failed-approach material.
    let output = valid.run(["failures", "exclude", "cron", "sessions"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        !text.contains(&abandoned[..12]),
        "a subject-only revert was linked through shared paths:\n{text}"
    );
    // An intervening path touch is irrelevant now: there is no link to reject.
    let rejected = TestRepo::new();
    let abandoned = rejected.commit_files_at(
        &[
            ("src/db/index.rs", b"cron sessions\n"),
            ("src/db/query.rs", b"cron sessions query\n"),
        ],
        "Exclude cron sessions from the FTS index",
        "2020-01-01T00:00:00+0000",
    );
    rejected.commit_at(
        "src/db/index.rs",
        b"cron sessions maintained\n",
        "Unrelated maintenance",
        "2020-01-15T00:00:00+0000",
    );
    rejected.commit_files_at(
        &[
            ("src/db/index.rs", b"cron sessions restored\n"),
            ("src/db/query.rs", b"cron sessions query restored\n"),
        ],
        "Revert the old database approach",
        "2020-02-01T00:00:00+0000",
    );
    rejected.index();
    let output = rejected.run(["failures", "exclude", "cron", "sessions"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        !text.contains(&abandoned[..12]),
        "the subject-only revert candidate swallowed the abandoned change:\n{text}"
    );
}

#[test]
fn failures_do_not_link_explicit_revert_of_another_commit_through_shared_paths() {
    // A changes only Cargo.lock; B changes the same file; a later revert explicitly
    // names B. The shared file must not attribute the revert to A.
    let repo = TestRepo::new();
    let a = repo.commit_files_at(
        &[("Cargo.lock", b"package a\n")],
        "Bump dependency for the parser",
        "2020-01-01T00:00:00+0000",
    );
    let b = repo.commit_files_at(
        &[("Cargo.lock", b"package a\npackage b\n")],
        "Pin the regression suite lockfile",
        "2020-02-01T00:00:00+0000",
    );
    repo.commit_files_at(
        &[("Cargo.lock", b"package a\n")],
        &format!(
            "Restore the lockfile\n\nThis reverts commit {}.\n",
            &b[..40]
        ),
        "2020-03-01T00:00:00+0000",
    );
    repo.index();
    let output = repo.run(["failures", "pin", "lockfile"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    // Only B is reported, as the revert's resolved target; A stays out entirely.
    let b_entry = block(&text, &b[..12]);
    assert!(
        b_entry.contains("recorded revert"),
        "the explicit revert of B was not reported against B:\n{b_entry}"
    );
    assert!(
        !text.contains(&a[..12]),
        "the revert of B was attributed to A through the shared lockfile:\n{text}"
    );
}

#[test]
fn failures_recognize_noncanonical_explicit_revert_declarations() {
    let repo = TestRepo::new();
    let abandoned = repo.commit_at(
        "src/db/index.rs",
        b"cron sessions\n",
        "Exclude cron sessions from the FTS index",
        "2020-01-01T00:00:00+0000",
    );
    let prefix = &abandoned[..12];
    let revert = repo.commit_at(
        "src/db/index.rs",
        b"cron sessions restored\n",
        &format!(
            "Revert broken FTS exclusion

             This is a revert of the code changes in commit
             {prefix} as it served no functional
             purpose."
        ),
        "2020-02-01T00:00:00+0000",
    );
    repo.index();
    let output = repo.run(["failures", "exclude", "cron", "sessions"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    let entry = block(&text, &abandoned[..12]);
    assert!(
        entry.contains(&revert[..12]) && entry.contains("recorded revert"),
        "a wrapped non-canonical declaration was not recognized:\n{entry}"
    );
}

#[test]
fn failures_ignore_conflicting_revert_declarations() {
    let repo = TestRepo::new();
    let first = repo.commit_at(
        "src/db/index.rs",
        b"cron sessions\n",
        "Exclude cron sessions from the FTS index",
        "2020-01-01T00:00:00+0000",
    );
    let second = repo.commit_at(
        "src/db/index.rs",
        b"cron sessions trimmed\n",
        "Trim cron sessions from the FTS index",
        "2020-01-02T00:00:00+0000",
    );
    repo.commit_at(
        "src/db/index.rs",
        b"cron sessions restored\n",
        &format!(
            "Revert the FTS exclusion

             This reverts commit {}.
             This reverts commit {}.",
            &first[..40],
            &second[..40]
        ),
        "2020-02-01T00:00:00+0000",
    );
    repo.index();
    let output = repo.run(["failures", "cron", "sessions"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    // Conflicting declarations resolve to nothing, so neither candidate carries the
    // revert; both stay unreported unless their own text alone qualifies them.
    assert!(
        !text.contains(&first[..12]),
        "conflicting declarations linked the revert to the first target:\n{text}"
    );
    assert!(
        !text.contains(&second[..12]),
        "conflicting declarations linked the revert to the second target:\n{text}"
    );
}
#[test]
fn failures_resolve_unique_abbreviated_revert_trailers() {
    let repo = TestRepo::new();
    let abandoned = repo.commit_at(
        "src/db/index.rs",
        b"cron sessions\n",
        "Exclude cron sessions from the FTS index",
        "2020-01-01T00:00:00+0000",
    );
    let prefix = &abandoned[..8];
    let revert = repo.commit_at(
        "src/db/index.rs",
        b"cron sessions restored\n",
        &format!(
            "Revert the old database approach\n\nThis reverts commit {prefix}.\n\nReason: the predicate was too broad."
        ),
        "2020-02-01T00:00:00+0000",
    );
    repo.index();

    let output = repo.run(["failures", "exclude", "cron", "sessions"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    let entry = block(&text, &abandoned[..12]);
    assert!(entry.contains(&revert[..12]), "{entry}");
    assert!(
        entry.contains("reason: Reason: the predicate was too broad"),
        "{entry}",
    );
}

#[test]
fn examples_and_failures_reject_paths_outside_the_repository() {
    let repo = provider_retirements();
    for path in ["/etc/passwd", "../../etc/passwd", "C:/Windows/system32"] {
        for command in ["examples", "failures"] {
            let output = repo.run([command, "provider", "--path", path]);
            assert_eq!(
                output.status.code(),
                Some(2),
                "{command} --path {path} should be invalid input: {}{}",
                stdout(&output),
                stderr(&output)
            );
            assert!(
                stdout(&output).is_empty(),
                "{command} --path {path} still returned material"
            );
        }
    }
}

#[test]
fn examples_and_failures_filter_candidates_before_limit_with_combined_scope() {
    let repo = TestRepo::new();
    let lower = repo.commit_at(
        "src/providers/boundary.rs",
        b"boundary example\n",
        "Retire telemetry provider at the lower boundary",
        "2020-01-01T00:00:00+0000",
    );
    let example = repo.commit_at(
        "src/providers/scoped.rs",
        b"release example\n",
        "Retire telemetry provider within the release",
        "2020-01-02T00:00:00+0000",
    );
    let failure = repo.commit_at(
        "src/db/noisy_cache.rs",
        b"filter enabled\n",
        "Exclude noisy cache tags from lookup",
        "2020-01-02T12:00:00+0000",
    );
    repo.commit_at(
        "src/db/noisy_cache.rs",
        b"filter disabled\n",
        &format!(
            "revert: exclude noisy cache tags from lookup\n\nThis reverts commit {failure}.\n"
        ),
        "2020-01-03T00:00:00+0000",
    );
    let mut upper = repo.head();

    // More matching commits than the unscoped candidate pool can inspect, all outside the
    // time window. They must not crowd in-scope material out before ranking and --limit.
    for index in 0..21 {
        let day = 10 + index;
        repo.commit_at(
            &format!("src/providers/outside-{index}.rs"),
            format!("outside example {index}\n").as_bytes(),
            &format!("Retire telemetry provider outside the release {index}"),
            &format!("2020-01-{day:02}T00:00:00+0000"),
        );
        let abandoned = repo.commit_at(
            "src/db/noisy_cache.rs",
            format!("filter enabled {index}\n").as_bytes(),
            &format!("Exclude noisy cache tags from lookup attempt {index}"),
            &format!("2020-01-{day:02}T01:00:00+0000"),
        );
        repo.commit_at(
            "src/db/noisy_cache.rs",
            b"filter disabled\n",
            &format!(
                "revert: exclude noisy cache tags from lookup\n\nThis reverts commit {abandoned}.\n"
            ),
            &format!("2020-01-{day:02}T02:00:00+0000"),
        );
        upper = repo.head();
    }
    repo.index();

    let examples = repo.run([
        "examples",
        "retire",
        "telemetry",
        "--from-rev",
        lower.as_str(),
        "--to-rev",
        upper.as_str(),
        "--since",
        "2020-01-02",
        "--until",
        "2020-01-03",
        "--limit",
        "1",
        "--json",
    ]);
    assert_eq!(examples.status.code(), Some(0), "{}", stderr(&examples));
    let examples: serde_json::Value = serde_json::from_slice(&examples.stdout).unwrap();
    assert_eq!(examples["materials"].as_array().unwrap().len(), 1);
    assert_eq!(examples["materials"][0]["citations"][0]["oid"], example);
    assert_eq!(
        examples["materials"][0]["paths"][0],
        "src/providers/scoped.rs"
    );
    assert_eq!(examples["scope"]["from_rev"], lower);
    assert_eq!(examples["scope"]["to_rev"], upper);
    assert_eq!(examples["scope"]["since"], "2020-01-02");
    assert_eq!(examples["scope"]["until"], "2020-01-03");

    let failures = repo.run([
        "failures",
        "exclude",
        "noisy",
        "cache",
        "--from-rev",
        lower.as_str(),
        "--to-rev",
        upper.as_str(),
        "--since",
        "2020-01-02",
        "--until",
        "2020-01-03",
        "--limit",
        "1",
        "--json",
    ]);
    assert_eq!(failures.status.code(), Some(0), "{}", stderr(&failures));
    let failures: serde_json::Value = serde_json::from_slice(&failures.stdout).unwrap();
    assert_eq!(failures["materials"].as_array().unwrap().len(), 1);
    assert_eq!(failures["materials"][0]["citations"][0]["oid"], failure);
    assert_eq!(
        failures["materials"][0]["paths"][0],
        "src/db/noisy_cache.rs"
    );
    assert_eq!(failures["scope"]["from_rev"], lower);
    assert_eq!(failures["scope"]["to_rev"], upper);
    assert_eq!(failures["scope"]["since"], "2020-01-02");
    assert_eq!(failures["scope"]["until"], "2020-01-03");
}

#[test]
fn examples_and_failures_share_scope_validation_and_empty_output() {
    let repo = provider_retirements();
    repo.index();
    let tip = repo.head();
    let lower = git_stdout(repo.dir.path(), ["rev-parse", "HEAD^"]);

    for (command, empty_message) in [
        ("examples", "No historical examples found."),
        ("failures", "No failed approaches found."),
    ] {
        let help = repo.run([command, "--help"]);
        assert_eq!(help.status.code(), Some(0), "{}", stderr(&help));
        let help = stdout(&help);
        for option in ["--from-rev", "--to-rev", "--since", "--until", "RFC 3339"] {
            assert!(
                help.contains(option),
                "{command} help missing {option}: {help}"
            );
        }

        let invalid_date = repo.run([command, "provider", "--since", "2024-02-30"]);
        assert_eq!(invalid_date.status.code(), Some(2));
        assert!(
            stderr(&invalid_date).contains("invalid --since value"),
            "{}",
            stderr(&invalid_date)
        );

        let reversed_revisions = repo.run([
            command,
            "provider",
            "--from-rev",
            tip.as_str(),
            "--to-rev",
            lower.as_str(),
        ]);
        assert_eq!(reversed_revisions.status.code(), Some(2));
        assert!(
            stderr(&reversed_revisions).contains("--from-rev must be an ancestor of --to-rev"),
            "{}",
            stderr(&reversed_revisions)
        );

        let empty = repo.run([command, "provider", "--since", "9999-12-31"]);
        assert_eq!(empty.status.code(), Some(0), "{}", stderr(&empty));
        let text = stdout(&empty);
        assert!(text.starts_with("Scope:"), "{text}");
        assert!(text.contains(empty_message), "{text}");

        let empty_json = repo.run([command, "provider", "--since", "9999-12-31", "--json"]);
        assert_eq!(empty_json.status.code(), Some(0), "{}", stderr(&empty_json));
        let empty_json: serde_json::Value = serde_json::from_slice(&empty_json.stdout).unwrap();
        assert_eq!(empty_json["materials"], serde_json::json!([]));
        assert_eq!(empty_json["scope"]["since"], "9999-12-31");
        assert_eq!(empty_json["scope"]["cache_tip"], tip);

        let unscoped = repo.run([command, "provider", "--json"]);
        assert_eq!(unscoped.status.code(), Some(0), "{}", stderr(&unscoped));
        let unscoped: serde_json::Value = serde_json::from_slice(&unscoped.stdout).unwrap();
        assert_eq!(unscoped["scope"]["to_rev"], tip);
        assert_eq!(unscoped["scope"]["cache_tip"], tip);
        assert_eq!(unscoped["scope"]["coverage_complete"], true);
    }
}
