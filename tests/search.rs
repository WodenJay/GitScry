mod support;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{fs, path::Path, process::Command};

use rusqlite::Connection;
use support::{TestRepo, git, git_command, git_stdout};

impl TestRepo {
    fn commit(&self, path: &str, contents: &[u8], message: &str) {
        if let Some(parent) = Path::new(path).parent() {
            fs::create_dir_all(self.dir.path().join(parent)).expect("create parent directory");
        }
        fs::write(self.dir.path().join(path), contents).expect("write tracked file");
        git(self.dir.path(), ["add", path]);
        git(self.dir.path(), ["commit", "-m", message]);
    }

    fn commit_at(&self, path: &str, contents: &[u8], message: &str, date: &str) {
        if let Some(parent) = Path::new(path).parent() {
            fs::create_dir_all(self.dir.path().join(parent)).expect("create parent directory");
        }
        fs::write(self.dir.path().join(path), contents).expect("write tracked file");
        git(self.dir.path(), ["add", path]);
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
    }
}

#[test]
fn search_reads_published_cache_and_reuses_it() {
    let repo = TestRepo::new();
    repo.commit(
        "src/providers/tavily.rs",
        b"legacy key redaction\n",
        "Retire TavilyProvider while preserving legacy key redaction",
    );
    repo.index();

    let first = repo.run(["search", "provider", "removal"]);
    assert_eq!(
        first.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let stdout = String::from_utf8_lossy(&first.stdout);
    assert!(stdout.contains("Retire TavilyProvider"));
    assert!(stdout.contains("src/providers/tavily.rs"));
    assert!(stdout.contains("confidence:"));
    assert!(stdout.contains("basis:"));
    assert!(stdout.contains(&repo.head()[..12]));
    assert!(first.stderr.is_empty());

    let second = repo.run(["search", "provider", "removal"]);
    assert_eq!(second.status.code(), Some(0));
    assert!(second.stderr.is_empty());
    assert_eq!(second.stdout, first.stdout);
}

#[test]
fn query_without_cache_reports_index_command_without_creating_artifacts() {
    let repo = TestRepo::new();

    let output = repo.run(["search", "provider"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("no published cache found; run `gitscry index` first")
    );
    assert!(!repo.dir.path().join(".gitscry").exists());
}

#[test]
fn query_rejects_a_damaged_published_cache_without_repairing_it() {
    let repo = TestRepo::new();
    repo.commit("history.txt", b"history\n", "History");
    repo.index();
    fs::write(
        repo.dir.path().join(".gitscry/cache.sqlite"),
        b"not a sqlite database",
    )
    .expect("damage published cache");

    let output = repo.run(["search", "history"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("published cache"));
    assert!(
        !fs::read_dir(repo.dir.path().join(".gitscry"))
            .unwrap()
            .flatten()
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with("cache.sqlite.corrupt-"))
    );
}

#[test]
fn index_rebuilds_stale_schema_without_preserving_it_as_corrupt() {
    let repo = TestRepo::new();
    repo.commit("history.txt", b"history\n", "History");
    repo.index();

    let cache = Connection::open(repo.dir.path().join(".gitscry/cache.sqlite")).unwrap();
    cache
        .execute(
            "UPDATE metadata SET value = '3' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
    drop(cache);

    let output = repo.run(["index"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(
        !fs::read_dir(repo.dir.path().join(".gitscry"))
            .unwrap()
            .flatten()
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("cache.sqlite.corrupt-")
            })
    );
    let search = repo.run(["search", "history"]);
    assert_eq!(search.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&search.stdout).contains("History"));
}

#[test]
fn query_rejects_a_stale_schema_with_index_instruction() {
    let repo = TestRepo::new();
    repo.commit("history.txt", b"history\n", "History");
    repo.index();

    let cache = Connection::open(repo.dir.path().join(".gitscry/cache.sqlite")).unwrap();
    cache
        .execute(
            "UPDATE metadata SET value = '3' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
    drop(cache);

    let output = repo.run(["search", "history"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("run `gitscry index`"));
}

#[test]
fn query_rejects_missing_completion_metadata_without_repairing_it() {
    let repo = TestRepo::new();
    repo.commit("history.txt", b"history\n", "History");
    repo.index();

    let connection =
        Connection::open(repo.dir.path().join(".gitscry/cache.sqlite")).expect("open cache");
    connection
        .execute("DELETE FROM metadata WHERE key = 'completed_tip'", [])
        .expect("remove completion marker");
    drop(connection);

    let output = repo.run(["search", "history"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("completion metadata is invalid"));
}

#[test]
fn query_finds_the_published_cache_from_a_nested_working_directory() {
    let repo = TestRepo::new();
    repo.commit("history.txt", b"history\n", "History");
    repo.index();
    let nested = repo.dir.path().join("src/deep");
    fs::create_dir_all(&nested).expect("create nested directory");

    let output = TestRepo::run_at(&nested, ["search", "history"]);

    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("History"));
}

#[test]
fn search_rebuilds_cache_when_default_tip_changes() {
    let repo = TestRepo::new();
    repo.commit("history.txt", b"first\n", "Initial history");

    repo.index();
    let first = repo.run(["search", "initial"]);
    assert_eq!(first.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&first.stdout).contains("Initial history"));

    repo.commit("history.txt", b"second\n", "Second history");
    let stale = repo.run(["search", "second"]);
    assert_eq!(stale.status.code(), Some(0));
    assert_eq!(stale.stdout, b"No relevant history found.\n");
    assert!(stale.stderr.is_empty());

    repo.index();
    let refreshed = repo.run(["search", "second"]);
    assert_eq!(refreshed.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&refreshed.stdout).contains("Second history"));
    assert!(refreshed.stderr.is_empty());
}

#[test]
fn search_refreshes_cache_after_shallow_history_deepens() {
    let source = TestRepo::new();
    source.commit("history.txt", b"one\n", "Commit one");
    source.commit("history.txt", b"two\n", "Commit two");
    source.commit("history.txt", b"three\n", "Commit three");

    let parent = tempfile::tempdir().expect("create clone parent");
    let clone = parent.path().join("clone");
    let cloned = git_command(parent.path())
        .args(["clone", "--depth", "1", "--no-local"])
        .arg(source.dir.path())
        .arg(&clone)
        .output()
        .expect("clone shallow repository");
    assert!(
        cloned.status.success(),
        "{}",
        String::from_utf8_lossy(&cloned.stderr)
    );

    let indexed = TestRepo::run_at(&clone, ["index"]);
    assert!(
        indexed.status.success(),
        "index failed: {}",
        String::from_utf8_lossy(&indexed.stderr)
    );

    let first = Command::new(env!("CARGO_BIN_EXE_gitscry"))
        .args(["search", "commit", "one"])
        .current_dir(&clone)
        .output()
        .expect("search shallow repository");
    assert_eq!(first.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&first.stderr).contains("local history is shallow"));
    assert!(!String::from_utf8_lossy(&first.stdout).contains("Commit one"));
    let code_first = TestRepo::run_at(&clone, ["search", "--code", "one"]);
    assert_eq!(code_first.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&code_first.stderr).contains("local history is shallow"));
    assert_eq!(code_first.stdout, b"No matching changed lines found.\n");

    git(&clone, ["fetch", "--deepen=2"]);

    let refreshed = TestRepo::run_at(&clone, ["index"]);
    assert!(
        refreshed.status.success(),
        "re-index failed: {}",
        String::from_utf8_lossy(&refreshed.stderr)
    );

    let second = Command::new(env!("CARGO_BIN_EXE_gitscry"))
        .args(["search", "commit", "one"])
        .current_dir(&clone)
        .output()
        .expect("search deepened repository");
    assert_eq!(second.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&second.stdout).contains("Commit one"));
    assert!(!String::from_utf8_lossy(&second.stderr).contains("Indexing local history"));
}

#[test]
fn search_replays_issue_2_secret_redaction_case() {
    let repo = TestRepo::new();
    repo.commit(
        "src/logging/redaction.rs",
        b"redact api keys before log output\n",
        "Redact API keys before log output",
    );

    let exact = git_stdout(
        repo.dir.path(),
        [
            "log",
            "--format=%H",
            "--grep=prevent API key leakage in log output",
        ],
    );
    assert!(exact.is_empty());
    repo.index();

    let output = repo.run(["search", "prevent API key leakage in log output"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Redact API keys before log output"));
    assert!(stdout.contains("src/logging/redaction.rs"));
    assert!(output.stderr.is_empty());
}

#[test]
fn search_uses_damaged_fts_without_rebuilding_cache() {
    let repo = TestRepo::new();
    repo.commit("provider.txt", b"provider\n", "Provider history");

    repo.index();
    let first = repo.run(["search", "provider"]);
    assert_eq!(first.status.code(), Some(0));

    let cache = Connection::open(repo.dir.path().join(".gitscry/cache.sqlite")).unwrap();
    cache
        .execute(
            "INSERT INTO search_fts(search_fts) VALUES ('delete-all')",
            [],
        )
        .unwrap();
    drop(cache);

    let second = repo.run(["search", "provider"]);
    assert_eq!(second.status.code(), Some(0));
    assert_eq!(second.stdout, b"No relevant history found.\n");
    assert!(second.stderr.is_empty());
    assert!(
        !fs::read_dir(repo.dir.path().join(".gitscry"))
            .unwrap()
            .flatten()
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("cache.sqlite.corrupt-")
            })
    );
}

#[test]
fn search_recognizes_bare_repository_filename() {
    let repo = TestRepo::new();
    repo.commit(
        "src/providers/tavily.rs",
        b"provider\n",
        "Update provider integration",
    );

    repo.index();
    let output = repo.run(["search", "tavily.rs"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("exact repository term"));
    assert!(stdout.contains("exact path match"));
}

#[test]
fn search_accepts_quoted_query_and_reports_no_result() {
    let repo = TestRepo::new();
    repo.commit("provider.txt", b"provider\n", "Retire provider safely");
    repo.index();

    let quoted = repo.run(["search", "provider safely"]);
    assert_eq!(quoted.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&quoted.stdout).contains("Retire provider safely"));
    let split = repo.run(["search", "provider", "safely"]);
    assert_eq!(split.status.code(), Some(0));
    assert_eq!(quoted.stdout, split.stdout);
    assert!(split.stderr.is_empty());

    let no_result = repo.run(["search", "term-that-does-not-exist"]);
    assert_eq!(no_result.status.code(), Some(0));
    assert_eq!(no_result.stdout, b"No relevant history found.\n");
    assert!(no_result.stderr.is_empty());
}

#[test]
fn search_limit_truncates_results_and_invalid_input_exits_two() {
    let repo = TestRepo::new();
    for (index, message) in [
        "Provider migration one",
        "Provider migration two",
        "Provider migration three",
    ]
    .into_iter()
    .enumerate()
    {
        repo.commit(
            &format!("provider-{index}.txt"),
            format!("provider {index}\n").as_bytes(),
            message,
        );
    }
    repo.index();

    let limited = repo.run(["search", "provider", "--limit", "1"]);
    assert_eq!(limited.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&limited.stdout);
    assert_eq!(stdout.matches("confidence:").count(), 1);
    assert!(stdout.contains("results truncated"));
    assert!(stdout.contains("Showing 1 of 3"));

    assert!(stdout.contains("matching commits"));
    let missing = repo.run(["search"]);
    assert_eq!(missing.status.code(), Some(2));
    let invalid_limit = repo.run(["search", "provider", "--limit", "0"]);
    assert_eq!(invalid_limit.status.code(), Some(2));
}

#[test]
fn search_orders_equal_matches_by_full_oid_and_abbreviates_uniquely() {
    let repo = TestRepo::new();
    for contents in [b"one\n", b"two\n", b"red\n"] {
        repo.commit_at(
            "provider.txt",
            contents,
            "Provider update",
            "2000-01-01T00:00:00+0000",
        );
    }
    repo.index();

    let output = repo.run(["search", "provider", "--limit", "3"]);
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8_lossy(&output.stdout);
    let ids = text
        .lines()
        .filter_map(|line| line.strip_prefix("- "))
        .map(|line| line.split_whitespace().next().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(ids.len(), 3);
    assert!(ids.iter().all(|id| id.len() >= 12));
    assert_eq!(ids.windows(2).filter(|pair| pair[0] >= pair[1]).count(), 0);
}

#[test]
fn code_search_matches_literal_changed_lines_but_not_unchanged_context() {
    let repo = TestRepo::new();
    repo.commit_at(
        "README.md",
        b"fn sample() {\n    let context = \"Needle[?]\";\n    let target = \"Needle[?] Needle[?]\";\n    let decoy = \"Needle?\";\n}\n",
        "Add documented example",
        "2000-01-01T00:00:00+0000",
    );
    let root_oid = repo.head();
    repo.commit_at(
        "README.md",
        b"fn sample() {\n    let context = \"Needle[?]\";\n    let target = \"changed Needle[?]\";\n    let decoy = \"Needle?\";\n}\n",
        "Update documented example",
        "2000-01-02T00:00:00+0000",
    );
    let changed_oid = repo.head();
    repo.index();

    let output = repo.run(["search", "--code", "Needle[?]"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let rows = stdout
        .lines()
        .filter(|line| line.starts_with("- "))
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 4, "{stdout}");
    let changed_prefix = format!("- {changed_oid} ");
    let changed_rows = rows
        .iter()
        .filter(|line| line.starts_with(&changed_prefix))
        .collect::<Vec<_>>();
    assert_eq!(changed_rows.len(), 2, "{stdout}");
    assert!(changed_rows[0].contains("README.md:3 removed:"));
    assert!(changed_rows[0].ends_with("let target = \"Needle[?] Needle[?]\";"));
    assert!(changed_rows[1].contains("README.md:3 added:"));
    assert!(changed_rows[1].ends_with("let target = \"changed Needle[?]\";"));
    assert!(!stdout.contains("let decoy"));
    assert!(
        rows.iter()
            .any(|line| line.starts_with(&format!("- {root_oid} ")))
    );

    let wrong_case = repo.run(["search", "--code", "needle[?]"]);
    assert_eq!(wrong_case.status.code(), Some(0));
    assert_eq!(wrong_case.stdout, b"No matching changed lines found.\n");
    let quoted = repo.run(["search", "--code", "Needle[?] Needle"]);
    assert_eq!(quoted.status.code(), Some(0));
    let quoted_rows = String::from_utf8_lossy(&quoted.stdout)
        .lines()
        .filter(|line| line.starts_with("- "))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(quoted_rows.len(), 2);
    assert!(quoted_rows[0].starts_with(&format!("- {changed_oid} ")));
    assert!(quoted_rows[0].contains("README.md:3 removed:"));
    assert!(quoted_rows[1].starts_with(&format!("- {root_oid} ")));
    assert!(quoted_rows[1].ends_with("let target = \"Needle[?] Needle[?]\";"));
}

#[test]
fn code_search_filters_historical_paths_and_limits_changed_lines() {
    let repo = TestRepo::new();
    repo.commit_at(
        "README.md",
        b"API example\nold_api(\"value\");\nkeep_one();\nkeep_two();\n",
        "Add API note",
        "2000-01-01T00:00:00+0000",
    );
    let root_oid = repo.head();
    repo.commit_at(
        "notes.txt",
        b"example api(\"docs\");\n",
        "Add text-file example",
        "2000-01-02T00:00:00+0000",
    );
    let note_oid = repo.head();

    fs::create_dir_all(repo.dir.path().join("src")).expect("create destination directory");
    git(repo.dir.path(), ["mv", "README.md", "src/new.rs"]);
    fs::write(
        repo.dir.path().join("src/new.rs"),
        b"API example\nnew_api(\"value\");\nkeep_one();\nkeep_two();\n",
    )
    .expect("update renamed file");
    git(repo.dir.path(), ["add", "--all"]);
    commit_staged_at(&repo, "Rename API note", "2000-01-02T00:00:00+0000");
    let rename_oid = repo.head();

    git(repo.dir.path(), ["rm", "src/new.rs"]);
    commit_staged_at(&repo, "Delete API note", "2000-01-03T00:00:00+0000");
    let deletion_oid = repo.head();
    repo.index();

    let limited = repo.run(["search", "--code", "api(", "--limit", "2"]);
    assert_eq!(limited.status.code(), Some(0));
    let text = String::from_utf8_lossy(&limited.stdout);
    assert!(text.contains("Showing 2 of 5 matching lines; results truncated."));
    let rows = text
        .lines()
        .filter_map(|line| line.strip_prefix("- "))
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 2, "{text}");
    assert!(rows[0].starts_with(&format!("{deletion_oid} src/new.rs:2 removed:")));
    let first_tied_oid = if note_oid < rename_oid {
        &note_oid
    } else {
        &rename_oid
    };
    assert!(rows[1].starts_with(&format!("{first_tied_oid} ")));

    let all = repo.run(["search", "--code", "api("]);
    assert_eq!(all.status.code(), Some(0));
    let all_text = String::from_utf8_lossy(&all.stdout);
    let all_rows = all_text
        .lines()
        .filter_map(|line| line.strip_prefix("- "))
        .collect::<Vec<_>>();
    assert_eq!(all_rows.len(), 5, "{all_text}");
    let rename_rows = all_rows
        .iter()
        .filter(|line| line.starts_with(&format!("{rename_oid} ")))
        .collect::<Vec<_>>();
    assert_eq!(rename_rows.len(), 2);
    assert!(rename_rows[0].contains("README.md:2 removed:"));
    assert!(rename_rows[1].contains("src/new.rs:2 added:"));
    assert!(
        all_rows
            .last()
            .unwrap()
            .starts_with(&format!("{root_oid} README.md:2 added:"))
    );

    for (args, expected) in [
        (
            [
                "search",
                "--code",
                "api(",
                "--change",
                "removed",
                "--path",
                "README.md",
            ],
            format!("{rename_oid} README.md:2 removed:"),
        ),
        (
            [
                "search",
                "--code",
                "api(",
                "--change",
                "added",
                "--path",
                "src/new.rs",
            ],
            format!("{rename_oid} src/new.rs:2 added:"),
        ),
        (
            [
                "search",
                "--code",
                "api(",
                "--change",
                "removed",
                "--path",
                "src/new.rs",
            ],
            format!("{deletion_oid} src/new.rs:2 removed:"),
        ),
        (
            [
                "search",
                "--code",
                "api(",
                "--change",
                "added",
                "--path",
                "README.md",
            ],
            format!("{root_oid} README.md:2 added:"),
        ),
    ] {
        let filtered = repo.run(args);
        assert_eq!(filtered.status.code(), Some(0));
        let filtered_text = String::from_utf8_lossy(&filtered.stdout);
        assert!(filtered_text.contains(&expected), "{filtered_text}");
        assert_eq!(filtered_text.matches("\n- ").count(), 1, "{filtered_text}");
    }

    let prefix_path = repo.run(["search", "--code", "api(", "--path", "src/new"]);
    assert_eq!(prefix_path.status.code(), Some(0));
    assert_eq!(prefix_path.stdout, b"No matching changed lines found.\n");
}

#[test]
fn code_search_json_and_human_output_escape_arbitrary_line_bytes() {
    let repo = TestRepo::new();
    let invalid_line = b"prefix needle-\xff";
    let mut contents = invalid_line.to_vec();
    contents.push(b'\n');
    contents.extend_from_slice(
        "separator needle\u{2028}tail\nparagraph needle\u{2029}tail\n".as_bytes(),
    );
    repo.commit("README.md", &contents, "Add text containing unusual bytes");
    let text_oid = repo.head();
    repo.commit("assets/image.bin", b"\0needle\0\n", "Add binary asset");
    repo.index();

    let json = repo.run(["search", "--code", "needle", "--json"]);
    assert_eq!(json.status.code(), Some(0));
    assert!(json.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(value["kind"], "code-search");
    assert_eq!(value["materials"], serde_json::json!([]));
    assert_eq!(value["matched_count"], 3);
    let matches = value["code_matches"].as_array().unwrap();
    assert_eq!(matches.len(), 3);
    assert_eq!(matches[0]["commit_id"], text_oid);
    assert_eq!(matches[0]["path"], "README.md");
    assert_eq!(matches[0]["direction"], "added");
    assert_eq!(matches[0]["line_number"], 1);
    assert_eq!(matches[0]["line"]["base64"], STANDARD.encode(invalid_line));
    assert_eq!(matches[1]["line_number"], 2);
    assert_eq!(matches[1]["line"], "separator needle\u{2028}tail");
    assert_eq!(matches[2]["line_number"], 3);
    assert_eq!(matches[2]["line"], "paragraph needle\u{2029}tail");

    let human = repo.run(["search", "--code", "needle"]);
    assert_eq!(human.status.code(), Some(0));
    let human_text = String::from_utf8_lossy(&human.stdout);
    assert!(human_text.contains(&format!("base64:{}", STANDARD.encode(invalid_line))));
    assert!(human_text.contains("\\u{2028}"));
    assert!(human_text.contains("\\u{2029}"));
    assert!(!human_text.contains('\u{2028}'));
    assert!(!human_text.contains('\u{2029}'));
    assert!(!human_text.contains("assets/image.bin"));

    let empty = repo.run(["search", "--code", "absent", "--json"]);
    assert_eq!(empty.status.code(), Some(0));
    let empty_value: serde_json::Value = serde_json::from_slice(&empty.stdout).unwrap();
    assert_eq!(empty_value["kind"], "code-search");
    assert_eq!(empty_value["code_matches"], serde_json::json!([]));
}

#[test]
fn code_search_rejects_missing_empty_multiline_and_mixed_modes() {
    let repo = TestRepo::new();
    for output in [
        repo.run(["search"]),
        repo.run(["search", "--code", ""]),
        repo.run(["search", "--code", "first\nsecond"]),
        repo.run(["search", "ordinary", "--code", "code"]),
        repo.run(["search", "--change", "added"]),
        repo.run(["search", "--path", "src/lib.rs"]),
    ] {
        assert_eq!(
            output.status.code(),
            Some(2),
            "{}",
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

#[test]
fn code_search_uses_first_parent_for_merge_changes() {
    let repo = TestRepo::new();
    repo.commit("merge.txt", b"base\nseparator\nshared\n", "Base");
    git(repo.dir.path(), ["switch", "-c", "feature"]);
    repo.commit("merge.txt", b"base\nseparator\nfeature\n", "Feature update");
    git(repo.dir.path(), ["switch", "main"]);
    repo.commit("merge.txt", b"main\nseparator\nshared\n", "Main update");

    let merge = git_command(repo.dir.path())
        .args(["merge", "--no-ff", "--no-commit", "feature"])
        .output()
        .expect("merge feature branch");
    assert!(
        merge.status.success(),
        "merge failed (status {:?}); stdout: {}; stderr: {}",
        merge.status.code(),
        String::from_utf8_lossy(&merge.stdout),
        String::from_utf8_lossy(&merge.stderr)
    );
    fs::write(
        repo.dir.path().join("merge.txt"),
        b"main\nseparator\nfeature\nmerge-marker\n",
    )
    .expect("add merge-specific line");
    git(repo.dir.path(), ["add", "--all"]);
    commit_staged_at(
        &repo,
        "Merge feature with resolution",
        "2000-01-04T00:00:00+0000",
    );
    let merge_oid = repo.head();
    repo.index();

    let output = repo.run(["search", "--code", "merge-marker"]);
    assert_eq!(output.status.code(), Some(0));
    let rows = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.starts_with("- "))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        [format!("- {merge_oid} merge.txt:4 added: merge-marker")],
    );
}

fn commit_staged_at(repo: &TestRepo, message: &str, date: &str) {
    let output = git_command(repo.dir.path())
        .args(["commit", "-m", message])
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .expect("commit staged changes");
    assert!(
        output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
