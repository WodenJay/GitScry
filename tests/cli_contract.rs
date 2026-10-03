mod support;

use std::{fs, path::Path};

use support::{TestRepo, git, git_command};

const CONTRACT: &str = include_str!("fixtures/cli-contract.txt");

fn commit(repo: &TestRepo, files: &[(&str, &[u8])], subject: &str, body: &str, date: &str) {
    for (path, contents) in files {
        let path = Path::new(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(repo.dir.path().join(parent)).expect("create parent directory");
        }
        fs::write(repo.dir.path().join(path), contents).expect("write tracked file");
    }
    git(repo.dir.path(), ["add", "--all"]);
    let output = git_command(repo.dir.path())
        .args(["commit", "-m", subject, "-m", body])
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

fn escaped(bytes: &[u8]) -> String {
    bytes.escape_ascii().map(char::from).collect()
}

#[test]
fn ordinary_commands_have_a_byte_exact_cli_contract() {
    let repo = TestRepo::new();
    commit(
        &repo,
        &[
            (
                "src/lib.rs",
                b"pub fn provider(value: &str) -> &str {\n    value\n}\n",
            ),
            (
                "tests/provider.rs",
                b"#[test]\nfn provider_accepts_values() {}\n",
            ),
        ],
        "Add provider normalization path",
        "Keep provider values available to callers.",
        "2000-01-01T00:00:00+0000",
    );
    commit(
        &repo,
        &[
            (
                "src/lib.rs",
                b"pub fn provider(value: &str) -> &str {\n    value.trim()\n}\n",
            ),
            (
                "tests/provider.rs",
                b"#[test]\nfn provider_normalizes_values() {}\n",
            ),
        ],
        "Retire legacy provider safely",
        "Normalize provider values so that stale input does not leak to callers.",
        "2000-01-02T00:00:00+0000",
    );
    let retired = repo.head();
    commit(
        &repo,
        &[
            (
                "src/lib.rs",
                b"pub fn provider(value: &str) -> &str {\n    value\n}\n",
            ),
            (
                "tests/provider.rs",
                b"#[test]\nfn provider_accepts_values() {}\n",
            ),
        ],
        "Revert \"Retire legacy provider safely\"",
        &format!(
            "This reverts commit {retired}.\n\nReason: provider normalization caused a regression in legacy callers.\nRetry: restore normalization after callers migrate."
        ),
        "2000-01-03T00:00:00+0000",
    );
    commit(
        &repo,
        &[
            (
                "src/lib.rs",
                b"pub fn provider(value: &str) -> &str {\n    value.trim()\n}\n",
            ),
            (
                "tests/provider.rs",
                b"#[test]\nfn provider_regression_is_fixed() {}\n",
            ),
        ],
        "Fix provider regression after revert",
        "Fix provider regression by restoring normalization after callers migrate.",
        "2000-01-04T00:00:00+0000",
    );
    let indexed = repo.run(["index"]);
    assert_eq!(
        indexed.status.code(),
        Some(0),
        "index: {}",
        String::from_utf8_lossy(&indexed.stderr)
    );

    let commands: [(&str, &[&str], &str); 9] = [
        ("search", &["search", "provider"], "search"),
        (
            "examples",
            &["examples", "retire", "provider", "--path", "src/lib.rs"],
            "examples",
        ),
        (
            "failures",
            &["failures", "provider", "--path", "src/lib.rs"],
            "failures",
        ),
        ("related", &["related", "src/lib.rs"], "related"),
        ("tests", &["tests", "src/lib.rs"], "tests"),
        (
            "why",
            &["why", "src/lib.rs", "--line", "1", "--limit", "3"],
            "why",
        ),
        (
            "regression",
            &["regression", "provider regression", "--path", "src/lib.rs"],
            "regression",
        ),
        (
            "trace-fix",
            &["trace-fix", "HEAD", "--path", "src/lib.rs"],
            "trace-fix",
        ),
        ("timeline", &["timeline", "src/lib.rs"], "timeline"),
    ];

    let mut observed = String::new();
    for (name, args, _) in commands {
        let output = repo.run(args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        observed.push_str(&format!(
            "[{name}]\nstatus={:?}\nstdout={}\nstderr={}\n",
            output.status.code(),
            escaped(&output.stdout),
            escaped(&output.stderr),
        ));
    }
    for (name, args, expected_kind) in commands {
        let mut json_args = args.to_vec();
        json_args.push("--json");
        let output = repo.run(json_args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{name} --json: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: serde_json::Value =
            serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
                panic!(
                    "{name} emitted invalid JSON: {error}\n{}",
                    String::from_utf8_lossy(&output.stdout)
                )
            });
        let expected_schema = if name == "why" { 5 } else { 1 };
        assert_eq!(value["schema_version"], expected_schema, "{name}");
        assert_eq!(value["kind"], expected_kind, "{name}");
        assert!(value["warnings"].is_array(), "{name}");
        assert!(value["notices"].is_array(), "{name}");
        if name == "timeline" {
            assert!(
                value["target_revision"]
                    .as_str()
                    .is_some_and(|oid| oid.len() == 40)
            );
            assert!(value["path"].is_string());
            assert!(value["total"].as_u64().is_some());
            assert!(value["offset"].as_u64().is_some());
            assert!(value["limit"].as_u64().is_some());
            assert!(value["start"].as_u64().is_some());
            assert!(value["end"].as_u64().is_some());
            assert!(value["has_more"].is_boolean());
            let entries = value["entries"].as_array().unwrap();
            assert!(!entries.is_empty(), "timeline should contain entries");
            assert!(
                entries[0]["commit_id"]
                    .as_str()
                    .is_some_and(|oid| oid.len() == 40)
            );
            assert!(entries[0]["timestamp"].is_string());
            assert!(entries[0]["subject"].is_string());
            assert!(entries[0]["path"].is_string());
            assert!(entries[0]["change_type"].is_string());
        } else {
            assert!(value["matched_count"].as_u64().is_some(), "{name}");
            assert!(value["truncated"].is_boolean(), "{name}");
            if name == "why" {
                assert!(value["why"].is_object(), "why summary");
                assert!(value["why"]["attribution"]["state"].is_string());
                assert!(value["why"]["counts"].is_object());
                assert!(value["target_related_modifications"].is_array());
                assert!(value.get("materials").is_none());
                assert!(value.get("confidence").is_none());
                for modification in value["target_related_modifications"].as_array().unwrap() {
                    assert!(modification["oid"].is_string());
                    assert!(modification["basis"].is_array());
                    assert!(modification["paths"].is_array());
                    assert!(modification.get("confidence").is_none());
                }
            } else {
                assert!(value["materials"].is_array(), "{name}");
                let materials = value["materials"].as_array().unwrap();
                assert!(
                    !materials.is_empty(),
                    "{name} should have matching materials"
                );
                for material in materials {
                    assert!(material["subject"].is_string(), "{name}");
                    assert!(material["paths"].is_array(), "{name}");
                    assert!(material["confidence"].is_string(), "{name}");
                    assert!(material["basis"].is_array(), "{name}");
                    let citations = material["citations"].as_array().unwrap();
                    for citation in citations {
                        assert_eq!(citation["oid"].as_str().unwrap().len(), 40, "{name}");
                        assert!(citation["abbreviation"].is_string(), "{name}");
                        assert!(citation["subject"].is_string(), "{name}");
                    }
                }
                let expected_detail = match name {
                    "examples" => Some("steps"),
                    "failures" => Some("failure"),
                    "related" | "tests" => Some("relation"),
                    "trace-fix" => Some("trace_fix"),
                    _ => None,
                };
                if let Some(expected_detail) = expected_detail {
                    assert_eq!(
                        materials[0]["detail"]["type"].as_str(),
                        Some(expected_detail),
                        "{name}"
                    );
                }
            }
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        for diagnostic in value["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .chain(value["notices"].as_array().unwrap())
        {
            assert!(!stderr.contains(diagnostic.as_str().unwrap()), "{name}");
        }
        if name == "why" {
            assert!(!value["notices"].as_array().unwrap().is_empty());
        }
    }

    let limited = repo.run(["search", "provider", "--limit", "1", "--json"]);
    assert_eq!(limited.status.code(), Some(0));
    let limited: serde_json::Value = serde_json::from_slice(&limited.stdout).unwrap();
    assert_eq!(limited["matched_count"], 4);
    assert_eq!(limited["materials"].as_array().unwrap().len(), 1);
    assert!(limited["truncated"].as_bool().unwrap());

    let empty = repo.run(["search", "no-matching-material-733", "--json"]);
    assert_eq!(empty.status.code(), Some(0));
    let empty: serde_json::Value = serde_json::from_slice(&empty.stdout).unwrap();
    assert_eq!(empty["matched_count"], 0);
    assert_eq!(empty["materials"], serde_json::json!([]));

    let invalid = repo.run(["search", "--json"]);
    assert_eq!(invalid.status.code(), Some(2));
    assert!(invalid.stdout.is_empty());

    if std::env::var_os("GITSCRY_UPDATE_CLI_CONTRACT").is_some() {
        fs::write(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cli-contract.txt"),
            &observed,
        )
        .expect("write CLI contract");
    } else {
        assert_eq!(observed, CONTRACT);
    }
}

#[test]
fn json_flag_is_available_only_for_query_commands() {
    let repo = TestRepo::new();
    let root = repo.run(["--help"]);
    assert_eq!(root.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&root.stdout).contains("--json"));
    let root_help = String::from_utf8_lossy(&root.stdout);
    assert!(
        root_help.contains("timeline"),
        "root help should list timeline"
    );

    for name in [
        "search",
        "examples",
        "failures",
        "related",
        "tests",
        "why",
        "regression",
        "trace-fix",
        "timeline",
        "trace-removal",
    ] {
        let help = repo.run([name, "--help"]);
        assert_eq!(help.status.code(), Some(0), "{name}");
        assert!(
            String::from_utf8_lossy(&help.stdout).contains("--json"),
            "{name} help must document --json"
        );
    }
    let timeline_help = repo.run(["timeline", "--help"]);
    assert_eq!(timeline_help.status.code(), Some(0));
    let timeline_help = String::from_utf8_lossy(&timeline_help.stdout)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(timeline_help.contains("--at <REV>"));
    assert!(timeline_help.contains("--limit <LIMIT>"));
    assert!(timeline_help.contains("--offset <OFFSET>"));
    assert!(timeline_help.contains("--last"));
    for flag in ["--from-rev", "--to-rev", "--since", "--until"] {
        assert!(
            timeline_help.contains(flag),
            "timeline help should document {flag}"
        );
    }
    let why_help = repo.run(["why", "--help"]);
    assert_eq!(why_help.status.code(), Some(0));
    let why_help = String::from_utf8_lossy(&why_help.stdout)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    for flag in ["--from-rev", "--to-rev", "--since", "--until"] {
        assert!(why_help.contains(flag), "why help should document {flag}");
    }
    let search_help = repo.run(["search", "--help"]);

    let why_help_lowercase = why_help.to_ascii_lowercase();
    for phrase in [
        "target-line attribution",
        "starting line only",
        "standalone target-related modifications",
        "attribution is independent",
        "not a root-cause explanation",
    ] {
        assert!(
            why_help_lowercase.contains(phrase),
            "why help should describe {phrase}"
        );
    }
    assert_eq!(search_help.status.code(), Some(0));
    let search_help = String::from_utf8_lossy(&search_help.stdout)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(search_help.contains("case-sensitive literal substring in added or removed lines"));
    assert!(search_help.contains("--code-regex <PATTERN>"));
    assert!(search_help.contains("Look-around and backreferences are unsupported"));
    assert!(search_help.contains("16,384 UTF-8 bytes"));
    assert!(search_help.contains("bounds returned lines, not scan work"));
    assert!(search_help.contains("--code <TEXT>"));
    assert!(search_help.contains("--change <CHANGE>"));
    assert!(search_help.contains("--path <PATH>"));
    assert!(search_help.contains("case-sensitive literal substring"));
    assert!(search_help.contains("default branch"));
    assert!(search_help.contains("--github-links"));
    assert!(search_help.contains("--github-repo <OWNER/REPO>"));
    assert!(search_help.contains("Commit.associatedPullRequests"));
    assert!(search_help.contains("existing GitHub login"));
    assert!(search_help.contains("Coverage comes first"));
    assert!(search_help.contains("continuation pages"));
    assert!(search_help.contains("conservative starting values, not empirically optimized"));
    assert!(search_help.contains("15-second timeout, 20-request, 50-results-per-page"));
    assert!(search_help.contains("complete, partial, not queried, or failed"));
    assert!(search_help.contains(
        "Partial GraphQL data and local object/field errors preserve usable associations"
    ));
    assert!(search_help.contains(
        "global authentication, rate-limit, network, or timeout failures stop link fetching"
    ));
    assert!(search_help.contains("No automatic retries"));
    assert!(search_help.contains("PR number, title, URL, repository identity"));
    assert!(search_help.contains("does not audit or claim a minimum permission set"));
    assert!(search_help.contains("schema_version` is 1 by default, 2 for `--patch` alone, and 4 whenever `--github-links` is enabled, except `why` reports use version 5"));
    assert!(search_help.contains("Version 4 may also include the optional `patch` field"));
    assert!(
        search_help.contains(
            "inspect optional fields instead of inferring enabled options from the version"
        )
    );
    assert!(search_help.contains("cannot guarantee every PR containing a commit"));
    assert!(search_help.contains("omit many issue mentions"));
    assert!(search_help.contains("not proof of closure or causality"));
    assert!(
        search_help.contains("Missing data or permissions do not prove that no association exists")
    );
    assert!(search_help.contains("search does not contact GitHub"));
    assert!(search_help.contains("Examples:"));
    for name in ["regression", "trace-fix"] {
        let help = repo.run([name, "--help"]);
        assert_eq!(help.status.code(), Some(0), "{name}");
        let help = String::from_utf8_lossy(&help.stdout)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        for flag in [
            "--from-rev <REV>",
            "--to-rev <REV>",
            "--since <DATE_OR_TIMESTAMP>",
            "--until <DATE_OR_TIMESTAMP>",
        ] {
            assert!(help.contains(flag), "{name} help missing {flag}");
        }
        assert!(
            help.contains("committer time"),
            "{name} help must explain time scope"
        );
        assert!(help.contains("UTC"), "{name} help must explain UTC bounds");
        assert!(
            help.contains("Examples:"),
            "{name} help must include examples"
        );
    }
    let regression_help = repo.run(["regression", "--help"]);
    let regression_help = String::from_utf8_lossy(&regression_help.stdout);
    assert!(regression_help.contains("--good"));
    assert!(regression_help.contains("--bad"));
    assert!(regression_help.contains("suspect window"));
    let trace_help = repo.run(["trace-fix", "--help"]);
    let trace_help = String::from_utf8_lossy(&trace_help.stdout);
    assert!(trace_help.contains("FIX_REVISION"));
    assert!(trace_help.contains("fix target"));

    for name in ["index", "update"] {
        let help = repo.run([name, "--help"]);
        assert_eq!(help.status.code(), Some(0), "{name}");
        assert!(!String::from_utf8_lossy(&help.stdout).contains("--json"));

        let rejected = repo.run([name, "--json"]);
        assert_eq!(rejected.status.code(), Some(2), "{name}");
        assert!(rejected.stdout.is_empty(), "{name}");
    }
}

#[test]
fn index_help_documents_shared_cache_scope() {
    let repo = TestRepo::new();
    let help = repo.run(["index", "--help"]);
    assert_eq!(help.status.code(), Some(0));
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(help.contains("linked worktrees"));
    assert!(help.contains("Git common directory"));
    assert!(help.contains("gitscry/"));
    assert!(help.contains("current default branch tip"));
}

#[test]
fn json_preserves_unclipped_paths_steps_and_complete_citations() {
    let repo = TestRepo::new();
    let paths = (0..9)
        .map(|index| format!("src/module_{index}.rs"))
        .collect::<Vec<_>>();
    let files = paths
        .iter()
        .map(|path| (path.as_str(), b"pub fn provider() {}".as_slice()))
        .collect::<Vec<_>>();

    #[cfg(target_os = "linux")]
    {
        use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

        let path = Path::new(OsStr::from_bytes(b"src/bad-\xff.rs"));
        fs::create_dir_all(repo.dir.path().join("src")).expect("create source directory");
        fs::write(repo.dir.path().join(path), b"pub fn provider() {}")
            .expect("write non-UTF-8 path");
    }

    commit(
        &repo,
        &files,
        "Add provider integration modules",
        "Add provider integration across all modules.",
        "2000-01-01T00:00:00+0000",
    );
    let indexed = repo.run(["index"]);
    assert_eq!(indexed.status.code(), Some(0));

    let output = repo.run(["examples", "provider", "integration", "--json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let material = &value["materials"][0];
    let paths = material["paths"].as_array().unwrap();
    let steps = material["detail"]["steps"].as_array().unwrap();
    #[cfg(target_os = "linux")]
    let expected_count = 10;
    #[cfg(not(target_os = "linux"))]
    let expected_count = 9;
    assert_eq!(paths.len(), expected_count);
    assert_eq!(steps.len(), expected_count);
    let citations = material["citations"].as_array().unwrap();
    assert!(!citations.is_empty());
    assert!(
        citations
            .iter()
            .all(|citation| { citation["oid"].as_str().is_some_and(|oid| oid.len() == 40) })
    );

    #[cfg(target_os = "linux")]
    {
        let invalid_path = paths
            .iter()
            .find(|path| path["base64"].is_string())
            .expect("non-UTF-8 path is represented as Base64");
        assert_eq!(invalid_path["base64"], "c3JjL2JhZC3/LnJz");
    }
}
#[test]
fn material_commands_share_github_link_options_and_caveats() {
    let repo = TestRepo::new();
    for name in [
        "examples",
        "failures",
        "related",
        "tests",
        "regression",
        "why",
        "trace-fix",
        "timeline",
    ] {
        let output = repo.run([name, "--help"]);
        assert_eq!(output.status.code(), Some(0), "{name}");
        let help = String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        for detail in [
            "--github-links",
            "--github-repo <OWNER/REPO>",
            "authenticated `gh` CLI",
            "does not enable link fetching by itself",
            "coverage-first pagination",
            "15-second timeout",
            "20 API requests",
            "50 results per page",
            "200 deduplicated PR+issue objects",
            "conservative starting values, not empirically optimized",
            "complete, partial, not queried, or failed",
            "stop link fetching only",
            "No automatic retries",
            "missing or unreturned associations do not prove none exist",
            "navigation only",
            "do not change Git material",
            "do not promise command-specific benefits",
            "participant descriptions do not outweigh changes visible in Git",
        ] {
            assert!(help.contains(detail), "{name} help missing {detail}");
        }
        if name == "tests" {
            assert!(help.contains("test co-change history does not prove assertions exist"));
        }
        if name == "regression" {
            assert!(help.contains("regression suspects are not root-cause findings"));
        }
    }
}

#[test]
fn json_keeps_every_relation_citation_while_text_stays_clipped() {
    let repo = TestRepo::new();
    for index in 1..=7 {
        let provider = format!("pub fn provider() -> usize {{ {index} }}");
        let codec = format!("pub fn codec() -> usize {{ {index} }}");
        let subject = format!("Update provider codec {index}");
        let date = format!("2000-01-0{index}T00:00:00+0000");
        commit(
            &repo,
            &[
                ("src/provider.rs", provider.as_bytes()),
                ("src/codec.rs", codec.as_bytes()),
            ],
            &subject,
            "",
            &date,
        );
    }
    let indexed = repo.run(["index"]);
    assert_eq!(indexed.status.code(), Some(0));

    let json = repo.run(["related", "src/provider.rs", "--json"]);
    assert_eq!(json.status.code(), Some(0));
    let json: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    let material = &json["materials"][0];
    let citations = material["citations"].as_array().unwrap();
    assert_eq!(citations.len(), 7);
    assert!(
        citations
            .iter()
            .all(|citation| citation["oid"].as_str().is_some_and(|oid| oid.len() == 40))
    );

    let text = repo.run(["related", "src/provider.rs"]);
    assert_eq!(text.status.code(), Some(0));
    let text = String::from_utf8_lossy(&text.stdout);
    assert_eq!(text.matches("  supporting commit:").count(), 5);
    assert!(text.contains("... 2 more supporting commits"));
}

#[test]
fn json_contains_query_warnings_without_repeating_them_on_stderr() {
    let repo = TestRepo::new();
    commit(
        &repo,
        &[
            ("src/provider.rs", b"pub fn provider() {}"),
            ("tests/provider.rs", b"#[test] fn provider_works() {}"),
        ],
        "Add provider implementation and tests",
        "",
        "2000-01-01T00:00:00+0000",
    );
    fs::remove_file(repo.dir.path().join("tests/provider.rs")).expect("remove old test");
    commit(
        &repo,
        &[],
        "Remove obsolete provider test",
        "",
        "2000-01-02T00:00:00+0000",
    );
    let indexed = repo.run(["index"]);
    assert_eq!(indexed.status.code(), Some(0));

    let output = repo.run(["tests", "src/provider.rs", "--json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(value["warnings"].as_array().unwrap().iter().any(|warning| {
        warning
            .as_str()
            .unwrap()
            .contains("absent from the current worktree")
    }));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("absent from the current worktree"));
}
#[test]
fn json_surfaces_cache_completeness_warnings_without_stderr_duplication() {
    let repo = TestRepo::new();
    commit(
        &repo,
        &[("src/provider.rs", b"pub fn provider() {}")],
        "Add provider implementation",
        "",
        "2000-01-01T00:00:00+0000",
    );
    let shallow = repo.head();
    fs::write(repo.dir.path().join(".git/shallow"), format!("{shallow}\n"))
        .expect("mark repository shallow");
    let indexed = repo.run(["index"]);
    assert_eq!(
        indexed.status.code(),
        Some(0),
        "index: {}",
        String::from_utf8_lossy(&indexed.stderr)
    );

    let output = repo.run(["search", "provider", "--json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "search --json: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let warning = "warning: local history is shallow; cache material is incomplete.";
    assert!(
        value["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry == warning)
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains(warning));

    let text = repo.run(["search", "provider"]);
    assert_eq!(text.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&text.stderr).contains(warning));
}

#[test]
fn update_help_is_canonical_and_upgrade_is_an_exact_alias() {
    let repo = TestRepo::new();
    let update = repo.run(["update", "--help"]);
    let upgrade = repo.run(["upgrade", "--help"]);

    assert_eq!(update.status.code(), Some(0));
    assert_eq!(upgrade.status.code(), Some(0));
    assert_eq!(update.stderr, upgrade.stderr);
    assert_eq!(update.stdout, upgrade.stdout);
    assert!(String::from_utf8_lossy(&update.stdout).contains("Usage: gitscry update"));
    assert!(String::from_utf8_lossy(&update.stdout).contains("gitscry upgrade"));

    let root = repo.run(["--help"]);
    let root_help = String::from_utf8_lossy(&root.stdout);
    assert!(root_help.lines().any(|line| line.trim_start().starts_with("update ") && line.contains("Update GitScry")));
    assert!(
        !root_help
            .lines()
            .any(|line| line.trim_start().starts_with("upgrade "))
    );
}
