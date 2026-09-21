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

    let unanchored = repo.run(["examples", "retire", "provider"]);
    assert_eq!(unanchored.status.code(), Some(0));

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

    // Quoted and split natural language are the same request.
    let split = repo.run(["examples", "retire", "provider"]);
    assert_eq!(stdout(&split), stdout(&unanchored));
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
    let removed = repo.remove(
        "src/db/legacy_index.rs",
        "refactor(db): drop the unused legacy index module",
    );
    assert!(!repo.dir.path().join("src/db/legacy_index.rs").exists());
    assert!(!removed.is_empty());
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
    assert_eq!(stdout(&examples), "No historical examples found.\n");

    let failures = repo.run(["failures", "term-that-does-not-exist"]);
    assert_eq!(failures.status.code(), Some(0));
    assert_eq!(stdout(&failures), "No failed approaches found.\n");
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
    assert!(!repo.dir.path().join(".gitscry/cache.sqlite").exists());
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
fn failures_links_a_revert_whose_named_commit_is_absent() {
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
    let entry = block(&text, &abandoned[..12]);

    // The undone paths confirm the link without the trailer, but no message states why.
    assert!(entry.contains(&revert[..12]), "{entry}");
    assert!(entry.contains("reason: Reason unknown"), "{entry}");
    // The revert itself resolves to nothing, so it is not reported on its own.
    assert!(
        !text.contains(&format!(
            "
- {} ",
            &revert[..12]
        )),
        "an unresolved revert was reported as its own failed approach:
{text}"
    );
}

#[test]
fn failures_preserve_path_fallback_coverage_and_intervening_rejection() {
    let valid = TestRepo::new();
    let abandoned = valid.commit_files_at(
        &[
            ("src/db/index.rs", b"cron sessions\n"),
            ("src/db/query.rs", b"cron sessions query\n"),
        ],
        "Exclude cron sessions from the FTS index",
        "2020-01-01T00:00:00+0000",
    );
    let revert = valid.commit_files_at(
        &[
            ("src/db/index.rs", b"cron sessions restored\n"),
            ("src/db/query.rs", b"cron sessions query restored\n"),
        ],
        "Revert the old database approach",
        "2020-02-01T00:00:00+0000",
    );
    valid.index();
    let output = valid.run(["failures", "exclude", "cron", "sessions"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    let entry = block(&text, &abandoned[..12]);
    assert!(entry.contains(&revert[..12]), "{entry}");

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
    assert!(!stdout(&output).contains(&abandoned[..12]));
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
