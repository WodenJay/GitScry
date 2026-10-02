#[path = "support/mod.rs"]
mod support;

fn commit_material_files(
    repo: &support::TestRepo,
    subject: &str,
    body: &str,
    files: &[(&str, &str)],
) {
    for (path, contents) in files {
        let path = std::path::Path::new(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(repo.dir.path().join(parent)).unwrap();
        }
        std::fs::write(repo.dir.path().join(path), contents).unwrap();
    }
    support::git(repo.dir.path(), ["add", "--all"]);
    support::git(repo.dir.path(), ["commit", "-m", subject, "-m", body]);
}

fn indexed_material_repo() -> support::TestRepo {
    let repo = support::TestRepo::new();
    commit_material_files(
        &repo,
        "Add provider normalization path",
        "Keep provider values available to callers.",
        &[
            (
                "src/lib.rs",
                "pub fn provider(value: &str) -> &str { value }\n",
            ),
            (
                "tests/provider.rs",
                "#[test]\nfn provider_accepts_values() {}\n",
            ),
        ],
    );
    commit_material_files(
        &repo,
        "Retire legacy provider safely",
        "Normalize provider values so stale input does not leak to callers.",
        &[
            (
                "src/lib.rs",
                "pub fn provider(value: &str) -> &str { value.trim() }\n",
            ),
            (
                "tests/provider.rs",
                "#[test]\nfn provider_normalizes_values() {}\n",
            ),
        ],
    );
    let retired = repo.head();
    commit_material_files(
        &repo,
        "Revert \"Retire legacy provider safely\"",
        &format!(
            "This reverts commit {retired}.\n\nReason: provider regression.\nRetry: restore normalization after callers migrate."
        ),
        &[
            (
                "src/lib.rs",
                "pub fn provider(value: &str) -> &str { value }\n",
            ),
            (
                "tests/provider.rs",
                "#[test]\nfn provider_accepts_values() {}\n",
            ),
        ],
    );
    commit_material_files(
        &repo,
        "Fix provider regression after revert",
        "Restore normalization after callers migrate.",
        &[
            (
                "src/lib.rs",
                "pub fn provider(value: &str) -> &str { value.trim() }\n",
            ),
            (
                "tests/provider.rs",
                "#[test]\nfn provider_regression_is_fixed() {}\n",
            ),
        ],
    );
    repo.index();
    repo
}

const MATERIAL_COMMANDS: [(&str, &[&str]); 7] = [
    (
        "examples",
        &[
            "examples",
            "retire",
            "provider",
            "--path",
            "src/lib.rs",
            "--since",
            "2000-01-01",
            "--limit",
            "2",
            "--patch",
        ],
    ),
    (
        "failures",
        &[
            "failures",
            "provider",
            "--path",
            "src/lib.rs",
            "--since",
            "2000-01-01",
            "--limit",
            "2",
        ],
    ),
    (
        "related",
        &[
            "related",
            "src/lib.rs",
            "--since",
            "2000-01-01",
            "--limit",
            "2",
        ],
    ),
    (
        "tests",
        &[
            "tests",
            "src/lib.rs",
            "--since",
            "2000-01-01",
            "--limit",
            "2",
        ],
    ),
    (
        "why",
        &[
            "why",
            "src/lib.rs",
            "--line",
            "1",
            "--since",
            "2000-01-01",
            "--limit",
            "2",
            "--patch",
        ],
    ),
    (
        "regression",
        &[
            "regression",
            "provider",
            "regression",
            "--path",
            "src/lib.rs",
            "--since",
            "2000-01-01",
            "--limit",
            "2",
            "--patch",
        ],
    ),
    (
        "trace-fix",
        &[
            "trace-fix",
            "HEAD",
            "--path",
            "src/lib.rs",
            "--since",
            "2000-01-01",
            "--limit",
            "2",
            "--patch",
        ],
    ),
];

#[test]
fn github_options_allow_a_search_with_no_matches() {
    let repo = support::TestRepo::new();
    std::fs::write(repo.dir.path().join("example.txt"), "unrelated content").unwrap();
    support::git(repo.dir.path(), ["add", "example.txt"]);
    support::git(repo.dir.path(), ["commit", "-m", "unrelated"]);
    repo.index();

    let output = repo.run([
        "search",
        "MissingMarker",
        "--github-links",
        "--github-repo",
        "acme/widget",
        "--json",
    ]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 3);
    assert_eq!(value["github_links"]["status"], "complete");
    assert_eq!(value["github_links"]["repository"], serde_json::Value::Null);
    assert_eq!(value["github_links"]["issue_status"], "not_queried");
    assert_eq!(
        value["github_links"]["pull_requests"],
        serde_json::json!([])
    );
    assert_eq!(
        value["github_links"]["commit_associations"],
        serde_json::json!([])
    );
}
#[test]
fn github_repository_alone_preserves_the_existing_json_contract() {
    let repo = support::TestRepo::new();
    std::fs::write(repo.dir.path().join("example.txt"), "RepoOnlyMarker").unwrap();
    support::git(repo.dir.path(), ["add", "example.txt"]);
    support::git(repo.dir.path(), ["commit", "-m", "RepoOnlyMarker"]);
    repo.index();

    let baseline = repo.run(["search", "RepoOnlyMarker", "--json"]);
    let configured = repo.run([
        "search",
        "RepoOnlyMarker",
        "--github-repo",
        "acme/widget",
        "--json",
    ]);

    assert_eq!(baseline.status.code(), Some(0));
    assert_eq!(configured.status.code(), Some(0));
    assert_eq!(configured.stdout, baseline.stdout);
}

#[test]
fn invalid_github_repository_does_not_fail_the_git_search() {
    let repo = support::TestRepo::new();
    std::fs::write(repo.dir.path().join("example.txt"), "IdentityMarker").unwrap();
    support::git(repo.dir.path(), ["add", "example.txt"]);
    support::git(repo.dir.path(), ["commit", "-m", "IdentityMarker"]);
    repo.index();

    let baseline = repo.run(["search", "IdentityMarker", "--json"]);
    let enabled = repo.run([
        "search",
        "IdentityMarker",
        "--github-links",
        "--github-repo",
        "../bad",
        "--json",
    ]);

    assert_eq!(baseline.status.code(), Some(0));
    assert_eq!(
        enabled.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&enabled.stderr)
    );
    let baseline: serde_json::Value = serde_json::from_slice(&baseline.stdout).unwrap();
    let enabled: serde_json::Value = serde_json::from_slice(&enabled.stdout).unwrap();
    assert_eq!(enabled["materials"], baseline["materials"]);
    assert_eq!(enabled["github_links"]["status"], "failed");
    assert!(
        enabled["github_links"]["reason"]
            .as_str()
            .unwrap()
            .contains("Invalid GitHub repository")
    );
}

#[test]
fn remaining_material_queries_keep_git_output_with_explicit_link_options() {
    let repo = indexed_material_repo();

    for (name, args) in MATERIAL_COMMANDS {
        let mut baseline_args = args.to_vec();
        baseline_args.push("--json");
        let baseline_output = repo.run(baseline_args.clone());
        assert_eq!(baseline_output.status.code(), Some(0), "{name}");
        let baseline: serde_json::Value = serde_json::from_slice(&baseline_output.stdout).unwrap();
        assert!(
            !baseline["materials"].as_array().unwrap().is_empty(),
            "{name}"
        );

        let mut repository_only_args = args.to_vec();
        repository_only_args.extend(["--github-repo", "acme/widget", "--json"]);
        let repository_only = repo.run(repository_only_args);
        assert_eq!(repository_only.status.code(), Some(0), "{name}");
        assert_eq!(repository_only.stdout, baseline_output.stdout, "{name}");

        let mut enabled_args = args.to_vec();
        enabled_args.extend(["--github-links", "--github-repo", "../invalid", "--json"]);
        let enabled = repo.run(enabled_args);
        assert_eq!(enabled.status.code(), Some(0), "{name}");
        let mut value: serde_json::Value = serde_json::from_slice(&enabled.stdout).unwrap();
        assert_eq!(value["kind"], name, "{name}");
        assert_eq!(value["schema_version"], 3, "{name}");
        assert_eq!(value["github_links"]["status"], "failed", "{name}");

        let mut expected = Vec::<String>::new();
        for material in value["materials"].as_array().unwrap() {
            for citation in material["citations"].as_array().unwrap() {
                let sha = citation["oid"].as_str().unwrap();
                if !expected.iter().any(|seen| seen == sha) {
                    expected.push(sha.to_owned());
                }
            }
        }
        let actual = value["github_links"]["commit_associations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|association| association["commit_sha"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(actual, expected, "{name}: link only returned citations");
        assert!(actual.iter().all(|sha| sha.len() == 40), "{name}");

        value.as_object_mut().unwrap().remove("github_links");
        value["schema_version"] = baseline["schema_version"].clone();
        assert_eq!(value, baseline, "{name}: Git materials stay unchanged");
    }
}

#[test]
fn code_search_emits_deduplicated_link_status_without_changing_matches() {
    let repo = support::TestRepo::new();
    std::fs::write(
        repo.dir.path().join("example.txt"),
        "CodeNeedle first\nCodeNeedle second\nCodeNeedle third\n",
    )
    .unwrap();
    support::git(repo.dir.path(), ["add", "example.txt"]);
    support::git(repo.dir.path(), ["commit", "-m", "Add code markers"]);
    repo.index();

    let baseline = repo.run(["search", "--code", "CodeNeedle", "--json"]);
    let enabled = repo.run([
        "search",
        "--code",
        "CodeNeedle",
        "--github-links",
        "--github-repo",
        "../bad",
        "--json",
    ]);
    assert_eq!(baseline.status.code(), Some(0));
    assert_eq!(
        enabled.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&enabled.stderr)
    );
    let baseline: serde_json::Value = serde_json::from_slice(&baseline.stdout).unwrap();
    let enabled: serde_json::Value = serde_json::from_slice(&enabled.stdout).unwrap();
    assert_eq!(baseline["schema_version"], 1);
    assert_eq!(enabled["schema_version"], 3);
    assert_eq!(enabled["code_matches"], baseline["code_matches"]);
    assert_eq!(enabled["matched_count"], baseline["matched_count"]);
    assert_eq!(enabled["code_matches"].as_array().unwrap().len(), 3);
    assert_eq!(enabled["github_links"]["status"], "failed");
    let associations = enabled["github_links"]["commit_associations"]
        .as_array()
        .unwrap();
    assert_eq!(associations.len(), 1);
    assert_eq!(associations[0]["commit_sha"], repo.head());
    assert_eq!(associations[0]["status"], "failed");
}

#[test]
fn timeline_github_options_preserve_entries_and_only_the_explicit_repo_enables_fetching() {
    let repo = support::TestRepo::new();
    std::fs::write(repo.dir.path().join("history.txt"), "first").unwrap();
    support::git(repo.dir.path(), ["add", "history.txt"]);
    support::git(repo.dir.path(), ["commit", "-m", "first timeline entry"]);
    repo.index();

    let baseline = repo.run(["timeline", "history.txt", "--json"]);
    let configured = repo.run([
        "timeline",
        "history.txt",
        "--github-repo",
        "acme/widget",
        "--json",
    ]);
    let enabled = repo.run([
        "timeline",
        "history.txt",
        "--github-links",
        "--github-repo",
        "../bad",
        "--json",
    ]);

    assert_eq!(baseline.status.code(), Some(0));
    assert_eq!(configured.status.code(), Some(0));
    assert_eq!(configured.stdout, baseline.stdout);
    assert_eq!(enabled.status.code(), Some(0));
    let baseline: serde_json::Value = serde_json::from_slice(&baseline.stdout).unwrap();
    let enabled: serde_json::Value = serde_json::from_slice(&enabled.stdout).unwrap();
    assert_eq!(enabled["schema_version"], 3);
    assert_eq!(enabled["entries"], baseline["entries"]);
    assert_eq!(enabled["github_links"]["status"], "failed");
    assert_eq!(
        enabled["github_links"]["commit_associations"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[cfg(unix)]
mod unix {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        process::{Command, Output},
    };

    use super::support::{TestRepo, git};
    use tempfile::TempDir;

    struct FakeGh {
        directory: TempDir,
        log: PathBuf,
        response: String,
        exit_status: String,
    }

    impl FakeGh {
        fn new(response: &str) -> Self {
            let directory = tempfile::tempdir().expect("create fake gh directory");
            let executable = directory.path().join("gh");
            fs::write(
                &executable,
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$GITSCRY_GH_LOG\"\nprintf '%s\\n' \"$GITSCRY_GH_RESPONSE\"\nexit \"$GITSCRY_GH_EXIT_STATUS\"\n",
            )
            .expect("write fake gh");
            let mut permissions = fs::metadata(&executable).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&executable, permissions).unwrap();
            let log = directory.path().join("calls");
            Self {
                directory,
                log,
                response: response.to_owned(),
                exit_status: "0".to_owned(),
            }
        }

        fn with_exit_status(response: &str, exit_status: i32) -> Self {
            let mut gh = Self::new(response);
            gh.exit_status = exit_status.to_string();
            gh
        }

        fn run(&self, repo: &TestRepo, args: &[&str]) -> Output {
            let original_path = std::env::var_os("PATH").unwrap_or_default();
            let path = std::env::join_paths(
                std::iter::once(self.directory.path().to_path_buf())
                    .chain(std::env::split_paths(&original_path)),
            )
            .unwrap();
            Command::new(env!("CARGO_BIN_EXE_gitscry"))
                .args(args)
                .current_dir(repo.dir.path())
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", repo.dir.path().join("global-config"))
                .env("GITSCRY_GH_LOG", &self.log)
                .env("GITSCRY_GH_RESPONSE", &self.response)
                .env("GITSCRY_GH_EXIT_STATUS", &self.exit_status)
                .env("PATH", path)
                .output()
                .expect("run gitscry")
        }

        fn calls(&self) -> usize {
            fs::read_to_string(&self.log)
                .unwrap_or_default()
                .lines()
                .count()
        }
        fn log(&self) -> String {
            fs::read_to_string(&self.log).unwrap_or_default()
        }
    }

    fn commit(repo: &TestRepo, message: &str, contents: &str) {
        fs::write(repo.dir.path().join("history.txt"), contents).unwrap();
        git(repo.dir.path(), ["add", "history.txt"]);
        git(repo.dir.path(), ["commit", "-m", message]);
    }

    fn indexed_repo(marker: &str) -> TestRepo {
        let repo = TestRepo::new();
        commit(&repo, marker, marker);
        repo.index();
        repo
    }

    fn run_without_gh(repo: &TestRepo, args: &[&str]) -> Output {
        let path = tempfile::tempdir().expect("create isolated PATH");
        let original_path = std::env::var_os("PATH").unwrap_or_default();
        let git = std::env::split_paths(&original_path)
            .map(|directory| directory.join("git"))
            .find(|candidate| candidate.is_file())
            .expect("git executable is available on PATH");
        std::os::unix::fs::symlink(git, path.path().join("git")).unwrap();
        Command::new(env!("CARGO_BIN_EXE_gitscry"))
            .args(args)
            .current_dir(repo.dir.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", repo.dir.path().join("global-config"))
            .env("PATH", path.path())
            .output()
            .expect("run gitscry without gh on PATH")
    }
    fn response(has_next_page: bool, nodes: serde_json::Value) -> String {
        serde_json::json!({
            "data": {
                "repository": {
                    "object": {
                        "associatedPullRequests": {
                            "pageInfo": {"hasNextPage": has_next_page},
                            "nodes": nodes
                        }
                    }
                }
            }
        })
        .to_string()
    }

    fn pull_request_node() -> serde_json::Value {
        serde_json::json!({
            "number": 42,
            "title": "Shared \"PR\" \u{1b}[31m",
            "url": "https://github.com/acme/widget/pull/42",
            "repository": {"nameWithOwner": "acme/widget"}
        })
    }

    fn successful_response() -> String {
        response(false, serde_json::json!([pull_request_node()]))
    }

    #[test]
    fn repository_option_alone_never_invokes_gh() {
        let repo = indexed_repo("RepoOnlyMarker");
        let baseline = repo.run(["search", "RepoOnlyMarker", "--json"]);
        let gh = FakeGh::new(&successful_response());
        let configured = gh.run(
            &repo,
            &[
                "search",
                "RepoOnlyMarker",
                "--github-repo",
                "acme/widget",
                "--json",
            ],
        );
        assert_eq!(configured.status.code(), Some(0));
        assert_eq!(configured.stdout, baseline.stdout);
        assert_eq!(gh.calls(), 0);
    }

    #[test]
    fn search_without_matches_never_invokes_gh() {
        let repo = indexed_repo("ExistingMarker");
        let gh = FakeGh::new(&successful_response());
        // The empty revision interval prevents semantic ranking from returning the fixture commit.
        let output = gh.run(
            &repo,
            &[
                "search",
                "MissingMarker",
                "--from-rev",
                "HEAD",
                "--to-rev",
                "HEAD",
                "--github-links",
                "--github-repo",
                "acme/widget",
                "--json",
            ],
        );
        assert_eq!(output.status.code(), Some(0));
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["github_links"]["status"], "complete");
        assert_eq!(
            value["github_links"]["pull_requests"],
            serde_json::json!([])
        );
        assert_eq!(gh.calls(), 0);
    }

    #[test]
    fn missing_gh_is_reported_without_failing_git_search() {
        let repo = indexed_repo("MissingGhMarker");
        let output = run_without_gh(
            &repo,
            &[
                "search",
                "MissingGhMarker",
                "--github-links",
                "--github-repo",
                "acme/widget",
                "--json",
            ],
        );
        assert_eq!(output.status.code(), Some(0));
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["github_links"]["status"], "failed");
        assert!(
            value["github_links"]["reason"]
                .as_str()
                .unwrap()
                .contains("'gh' was not found")
        );
    }
    #[test]
    fn search_emits_deduplicated_pull_request_links_for_each_returned_commit() {
        let repo = TestRepo::new();
        git(
            repo.dir.path(),
            [
                "remote",
                "add",
                "origin",
                "https://github.com/acme/widget.git",
            ],
        );
        git(
            repo.dir.path(),
            ["remote", "add", "mirror", "git@github.com:acme/widget.git"],
        );
        commit(&repo, "LinkMarker first result", "first");
        commit(&repo, "LinkMarker second result", "second");
        repo.index();
        let baseline = repo.run([
            "search",
            "LinkMarker",
            "--since",
            "2000-01-01",
            "--limit",
            "2",
            "--patch",
            "--json",
        ]);
        assert_eq!(baseline.status.code(), Some(0));
        let baseline: serde_json::Value = serde_json::from_slice(&baseline.stdout).unwrap();
        let gh = FakeGh::new(&successful_response());

        let output = gh.run(
            &repo,
            &[
                "search",
                "LinkMarker",
                "--since",
                "2000-01-01",
                "--limit",
                "2",
                "--patch",
                "--github-links",
                "--json",
            ],
        );

        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!output.stdout.contains(&0x1b));
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let links = &value["github_links"];
        assert_eq!(value["matched_count"], baseline["matched_count"]);
        assert_eq!(value["truncated"], baseline["truncated"]);
        assert_eq!(value["materials"], baseline["materials"]);
        assert_eq!(value["warnings"], baseline["warnings"]);
        assert_eq!(value["notices"], baseline["notices"]);
        assert_eq!(value["scope"], baseline["scope"]);
        assert_eq!(baseline["schema_version"], 2);
        assert!(value["materials"][0]["patch"].is_object());
        assert_eq!(value["schema_version"], 3);
        assert_eq!(links["repository"], "acme/widget");
        assert_eq!(links["status"], "complete");
        assert_eq!(links["issue_status"], "not_queried");
        assert_eq!(links["pull_requests"].as_array().unwrap().len(), 1);
        assert_eq!(links["pull_requests"][0]["number"], 42);
        assert_eq!(links["pull_requests"][0]["type"], "pull_request");
        assert_eq!(
            links["pull_requests"][0]["title"],
            format!("Shared \"PR\" {}[31m", char::from(27))
        );
        assert_eq!(links["commit_associations"].as_array().unwrap().len(), 2);
        assert!(links["commit_associations"].as_array().unwrap().iter().all(
            |commit| commit["pull_request_urls"][0] == "https://github.com/acme/widget/pull/42"
        ));
        assert!(
            links["commit_associations"]
                .as_array()
                .unwrap()
                .iter()
                .all(|commit| commit["status"] == "complete"
                    && commit["commit_sha"]
                        .as_str()
                        .is_some_and(|sha| sha.len() == 40))
        );
        assert_eq!(gh.calls(), 2);
        assert!(gh.log().contains("associatedPullRequests(first: 50)"));
        assert!(gh.log().contains("owner=acme"));
        assert!(gh.log().contains("name=widget"));
        let human = gh.run(
            &repo,
            &[
                "search",
                "LinkMarker",
                "--github-links",
                "--github-repo",
                "acme/widget",
            ],
        );
        assert_eq!(human.status.code(), Some(0));
        assert!(!human.stdout.contains(&0x1b));
        assert!(String::from_utf8_lossy(&human.stdout).contains("Shared \"PR\""));
    }
    fn add_remote(repo: &TestRepo, name: &str, url: &str) {
        git(repo.dir.path(), ["remote", "add", name, url]);
    }

    #[test]
    fn empty_success_is_distinct_from_failure_and_unqueried_issues() {
        let repo = indexed_repo("EmptyMarker");
        let gh = FakeGh::new(&response(false, serde_json::json!([])));
        let output = gh.run(
            &repo,
            &[
                "search",
                "EmptyMarker",
                "--github-links",
                "--github-repo",
                "acme/widget",
                "--json",
            ],
        );
        assert_eq!(output.status.code(), Some(0));
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let links = &value["github_links"];
        assert_eq!(links["status"], "complete");
        assert_eq!(links["issue_status"], "not_queried");
        assert_eq!(links["pull_requests"], serde_json::json!([]));
        assert_eq!(links["commit_associations"][0]["status"], "complete");
        assert_eq!(
            links["commit_associations"][0]["pull_request_urls"],
            serde_json::json!([])
        );
        assert!(links["reason"].is_null());
        assert_eq!(gh.calls(), 1);
    }

    #[test]
    fn remaining_pull_request_pages_are_marked_partial() {
        let repo = indexed_repo("PagedMarker");
        let gh = FakeGh::new(&response(true, serde_json::json!([pull_request_node()])));
        let output = gh.run(
            &repo,
            &[
                "search",
                "PagedMarker",
                "--github-links",
                "--github-repo",
                "acme/widget",
                "--json",
            ],
        );
        assert_eq!(output.status.code(), Some(0));
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let links = &value["github_links"];
        assert_eq!(links["status"], "partial");
        assert_eq!(links["pull_requests"].as_array().unwrap().len(), 1);
        assert_eq!(links["commit_associations"][0]["status"], "partial");
        assert!(
            links["reason"]
                .as_str()
                .unwrap()
                .contains("later pages were not fetched")
        );
        assert_eq!(gh.calls(), 1);
    }

    #[test]
    fn partial_graphql_data_keeps_usable_pull_requests() {
        let repo = indexed_repo("PartialMarker");
        let mut response: serde_json::Value =
            serde_json::from_str(&response(false, serde_json::json!([pull_request_node()])))
                .unwrap();
        response["errors"] = serde_json::json!([{"message": "simulated partial response"}]);
        let gh = FakeGh::new(&response.to_string());
        let output = gh.run(
            &repo,
            &[
                "search",
                "PartialMarker",
                "--github-links",
                "--github-repo",
                "acme/widget",
                "--json",
            ],
        );
        assert_eq!(output.status.code(), Some(0));
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let links = &value["github_links"];
        assert_eq!(links["status"], "partial");
        assert_eq!(links["pull_requests"].as_array().unwrap().len(), 1);
        assert_eq!(links["commit_associations"][0]["status"], "partial");
        assert!(links["reason"].as_str().unwrap().contains("partial data"));
    }

    #[test]
    fn failed_gh_process_does_not_fail_search_or_leak_response_text() {
        let repo = indexed_repo("FailureMarker");
        let secret_marker = "AUTH_TOKEN_SHOULD_NOT_APPEAR";
        let response = format!(r#"{{"errors":[{{"message":"{secret_marker}"}}]}}"#);
        let gh = FakeGh::with_exit_status(&response, 1);
        let output = gh.run(
            &repo,
            &[
                "search",
                "FailureMarker",
                "--github-links",
                "--github-repo",
                "acme/widget",
                "--json",
            ],
        );
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!stdout.contains(secret_marker));
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["github_links"]["status"], "failed");
        assert_eq!(
            value["github_links"]["commit_associations"][0]["status"],
            "failed"
        );
        assert_eq!(gh.calls(), 1);
    }

    #[test]
    fn ambiguous_remotes_fail_only_the_association_lookup() {
        let repo = indexed_repo("AmbiguousMarker");
        add_remote(&repo, "origin", "https://github.com/acme/one.git");
        add_remote(&repo, "upstream", "git@github.com:acme/two.git");
        let baseline = repo.run(["search", "AmbiguousMarker", "--json"]);
        let gh = FakeGh::new(&successful_response());
        let output = gh.run(
            &repo,
            &["search", "AmbiguousMarker", "--github-links", "--json"],
        );
        assert_eq!(output.status.code(), Some(0));
        let baseline: serde_json::Value = serde_json::from_slice(&baseline.stdout).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["materials"], baseline["materials"]);
        assert_eq!(value["github_links"]["status"], "failed");
        assert!(
            value["github_links"]["reason"]
                .as_str()
                .unwrap()
                .contains("Multiple GitHub repositories")
        );
        assert_eq!(gh.calls(), 0);
    }

    #[test]
    fn explicit_repository_overrides_ambiguous_remotes() {
        let repo = indexed_repo("ExplicitMarker");
        add_remote(&repo, "origin", "https://github.com/acme/one.git");
        add_remote(&repo, "upstream", "git@github.com:acme/two.git");
        let gh = FakeGh::new(&successful_response());
        let output = gh.run(
            &repo,
            &[
                "search",
                "ExplicitMarker",
                "--github-links",
                "--github-repo",
                "acme/widget",
                "--json",
            ],
        );
        assert_eq!(output.status.code(), Some(0));
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["github_links"]["repository"], "acme/widget");
        assert_eq!(value["github_links"]["status"], "complete");
        assert!(gh.log().contains("owner=acme"));
        assert!(gh.log().contains("name=widget"));
        assert_eq!(gh.calls(), 1);
    }
    #[test]
    fn code_search_links_only_distinct_returned_commits() {
        let repo = TestRepo::new();
        commit(
            &repo,
            "Add code markers",
            "CodeNeedle first\nCodeNeedle second\nCodeNeedle third\n",
        );
        repo.index();

        let baseline = repo.run(["search", "--code", "CodeNeedle", "--json"]);
        assert_eq!(baseline.status.code(), Some(0));
        let baseline: serde_json::Value = serde_json::from_slice(&baseline.stdout).unwrap();
        let gh = FakeGh::new(&successful_response());
        let output = gh.run(
            &repo,
            &[
                "search",
                "--code",
                "CodeNeedle",
                "--github-links",
                "--github-repo",
                "acme/widget",
                "--json",
            ],
        );

        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(baseline["schema_version"], 1);
        assert_eq!(value["schema_version"], 3);
        assert_eq!(value["code_matches"], baseline["code_matches"]);
        assert_eq!(value["matched_count"], baseline["matched_count"]);
        assert_eq!(value["scope"], baseline["scope"]);
        assert_eq!(value["code_matches"].as_array().unwrap().len(), 3);
        assert_eq!(value["github_links"]["status"], "complete");
        assert_eq!(
            value["github_links"]["commit_associations"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            value["github_links"]["commit_associations"][0]["commit_sha"],
            repo.head()
        );
        assert_eq!(gh.calls(), 1);
    }

    #[test]
    fn timeline_links_only_the_scoped_current_page() {
        let repo = TestRepo::new();
        commit(&repo, "first timeline entry", "first");
        let first = repo.head();
        commit(&repo, "second timeline entry", "second");
        let second = repo.head();
        commit(&repo, "third timeline entry", "third");
        repo.index();

        let baseline = repo.run([
            "timeline",
            "history.txt",
            "--from-rev",
            &first,
            "--limit",
            "1",
            "--patch",
            "--json",
        ]);
        let gh = FakeGh::new(&successful_response());
        let output = gh.run(
            &repo,
            &[
                "timeline",
                "history.txt",
                "--from-rev",
                &first,
                "--limit",
                "1",
                "--patch",
                "--github-links",
                "--github-repo",
                "acme/widget",
                "--json",
            ],
        );
        assert_eq!(baseline.status.code(), Some(0));
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let baseline: serde_json::Value = serde_json::from_slice(&baseline.stdout).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(baseline["entries"].as_array().unwrap().len(), 1);
        assert_eq!(baseline["entries"][0]["commit_id"], second);
        assert_eq!(value["schema_version"], 3);
        assert_eq!(baseline["schema_version"], 2);
        assert_eq!(baseline["entries"][0]["patch"]["status"], "available");
        assert_eq!(value["entries"], baseline["entries"]);
        assert_eq!(value["total"], baseline["total"]);
        assert_eq!(value["offset"], baseline["offset"]);
        assert_eq!(value["scope"], baseline["scope"]);
        assert_eq!(
            value["github_links"]["commit_associations"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            value["github_links"]["commit_associations"][0]["commit_sha"],
            second
        );
        let human = gh.run(
            &repo,
            &[
                "timeline",
                "history.txt",
                "--from-rev",
                &first,
                "--limit",
                "1",
                "--patch",
                "--github-links",
                "--github-repo",
                "acme/widget",
            ],
        );
        assert_eq!(
            human.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&human.stderr)
        );
        let human = String::from_utf8_lossy(&human.stdout);
        assert!(human.contains("GitHub associations"));
        assert!(human.contains("pull request #42"));
        assert_eq!(gh.calls(), 2);
    }

    #[test]
    fn timeline_repository_alone_does_not_invoke_gh() {
        let repo = TestRepo::new();
        commit(&repo, "timeline without links", "content");
        repo.index();
        let baseline = repo.run(["timeline", "history.txt", "--json"]);
        let gh = FakeGh::new(&successful_response());
        let configured = gh.run(
            &repo,
            &[
                "timeline",
                "history.txt",
                "--github-repo",
                "acme/widget",
                "--json",
            ],
        );

        assert_eq!(configured.status.code(), Some(0));
        assert_eq!(configured.stdout, baseline.stdout);
        assert_eq!(gh.calls(), 0);
    }

    #[test]
    fn timeline_empty_page_does_not_invoke_gh() {
        let repo = TestRepo::new();
        commit(&repo, "timeline without links", "content");
        repo.index();
        let gh = FakeGh::new(&successful_response());
        let output = gh.run(
            &repo,
            &[
                "timeline",
                "history.txt",
                "--offset",
                "10",
                "--github-links",
                "--github-repo",
                "acme/widget",
                "--json",
            ],
        );

        assert_eq!(output.status.code(), Some(0));
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["schema_version"], 3);
        assert!(value["entries"].as_array().unwrap().is_empty());
        assert_eq!(value["github_links"]["status"], "complete");
        assert!(
            value["github_links"]["commit_associations"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(gh.calls(), 0);
    }
}

#[cfg(unix)]
mod remaining_material_links {
    use super::unix::{FakeGh, successful_response};
    use super::{MATERIAL_COMMANDS, indexed_material_repo};
    #[test]
    fn remaining_material_commands_fetch_links_for_returned_citations_only() {
        let repo = indexed_material_repo();
        let gh = FakeGh::new(&successful_response());

        for (name, args) in MATERIAL_COMMANDS {
            let mut baseline_args = args.to_vec();
            baseline_args.push("--json");
            let baseline_output = repo.run(baseline_args.clone());
            assert_eq!(baseline_output.status.code(), Some(0), "{name}");
            let baseline: serde_json::Value =
                serde_json::from_slice(&baseline_output.stdout).unwrap();

            let mut repository_only_args = args.to_vec();
            repository_only_args.extend(["--github-repo", "acme/widget", "--json"]);
            let repository_only = gh.run(&repo, &repository_only_args);
            assert_eq!(repository_only.status.code(), Some(0), "{name}");
            assert_eq!(repository_only.stdout, baseline_output.stdout, "{name}");
            assert_eq!(gh.calls(), 0, "{name}: repo selection must not fetch");

            let mut enabled_args = args.to_vec();
            enabled_args.extend(["--github-links", "--github-repo", "acme/widget", "--json"]);
            let output = gh.run(&repo, &enabled_args);
            assert_eq!(output.status.code(), Some(0), "{name}");
            let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(value["kind"], name, "{name}");
            assert_eq!(value["schema_version"], 3, "{name}");
            assert!(!value["materials"].as_array().unwrap().is_empty(), "{name}");

            let links = &value["github_links"];
            assert_eq!(links["status"], "complete", "{name}");
            assert_eq!(links["repository"], "acme/widget", "{name}");
            assert_eq!(
                links["pull_requests"].as_array().unwrap().len(),
                1,
                "{name}"
            );

            let mut expected = Vec::<String>::new();
            for material in value["materials"].as_array().unwrap() {
                for citation in material["citations"].as_array().unwrap() {
                    let sha = citation["oid"].as_str().unwrap();
                    if !expected.iter().any(|seen| seen == sha) {
                        expected.push(sha.to_owned());
                    }
                }
            }
            let actual = links["commit_associations"]
                .as_array()
                .unwrap()
                .iter()
                .map(|association| association["commit_sha"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>();
            assert_eq!(
                actual, expected,
                "{name}: preserve ordered unique citations"
            );

            let mut without_links = value.clone();
            without_links
                .as_object_mut()
                .unwrap()
                .remove("github_links");
            without_links["schema_version"] = baseline["schema_version"].clone();
            assert_eq!(
                without_links, baseline,
                "{name}: Git materials stay unchanged"
            );
        }
    }

    #[test]
    fn github_failures_do_not_change_any_remaining_material_query() {
        let repo = indexed_material_repo();
        let secret = "TOKEN_SHOULD_NOT_LEAK";
        let gh =
            FakeGh::with_exit_status(&format!(r#"{{"errors":[{{"message":"{secret}"}}]}}"#), 1);

        for (name, args) in MATERIAL_COMMANDS {
            let mut baseline_args = args.to_vec();
            baseline_args.push("--json");
            let baseline_output = repo.run(baseline_args);
            assert_eq!(baseline_output.status.code(), Some(0), "{name}");
            let baseline: serde_json::Value =
                serde_json::from_slice(&baseline_output.stdout).unwrap();

            let calls_before = gh.calls();
            let mut enabled_args = args.to_vec();
            enabled_args.extend(["--github-links", "--github-repo", "acme/widget", "--json"]);
            let output = gh.run(&repo, &enabled_args);
            assert_eq!(output.status.code(), Some(0), "{name}");
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(!stdout.contains(secret), "{name}: do not leak gh output");
            let mut value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(value["github_links"]["status"], "failed", "{name}");
            assert!(
                gh.calls() > calls_before,
                "{name}: returned citations should query gh"
            );

            value.as_object_mut().unwrap().remove("github_links");
            value["schema_version"] = baseline["schema_version"].clone();
            assert_eq!(
                value, baseline,
                "{name}: failed links must leave Git output unchanged"
            );
        }
    }
}
