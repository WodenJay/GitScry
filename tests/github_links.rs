#[path = "support/mod.rs"]
mod support;

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
    assert_eq!(value["schema_version"], 4);
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
        exit_status: String,
    }

    impl FakeGh {
        fn new(response: &str) -> Self {
            Self::with_responses(
                vec![response.to_owned()],
                vec![issue_response(false, serde_json::json!([]))],
            )
        }

        fn with_responses(
            pull_request_responses: Vec<String>,
            issue_responses: Vec<String>,
        ) -> Self {
            assert!(!pull_request_responses.is_empty());
            assert!(!issue_responses.is_empty());
            let directory = tempfile::tempdir().expect("create fake gh directory");
            let executable = directory.path().join("gh");
            fs::write(
                &executable,
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$GITSCRY_GH_LOG\"\ncase \"$*\" in\n  *closingIssuesReferences*)\n    response_file=\"$GITSCRY_GH_ISSUE_RESPONSES\"\n    call=$(grep -c closingIssuesReferences \"$GITSCRY_GH_LOG\")\n    ;;\n  *)\n    response_file=\"$GITSCRY_GH_PR_RESPONSES\"\n    call=$(grep -vc closingIssuesReferences \"$GITSCRY_GH_LOG\")\n    ;;\nesac\nresponse=$(sed -n \"${call}p\" \"$response_file\")\nif [ -z \"$response\" ]; then response=$(tail -n 1 \"$response_file\"); fi\nprintf '%s\\n' \"$response\"\nexit \"$GITSCRY_GH_EXIT_STATUS\"\n",
            )
            .expect("write fake gh");
            let mut permissions = fs::metadata(&executable).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&executable, permissions).unwrap();
            let log = directory.path().join("calls");
            fs::write(
                directory.path().join("pull-request-responses"),
                format!("{}\\n", pull_request_responses.join("\\n")),
            )
            .expect("write pull-request responses");
            fs::write(
                directory.path().join("issue-responses"),
                format!("{}\\n", issue_responses.join("\\n")),
            )
            .expect("write issue responses");
            Self {
                directory,
                log,
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
                .env(
                    "GITSCRY_GH_PR_RESPONSES",
                    self.directory.path().join("pull-request-responses"),
                )
                .env(
                    "GITSCRY_GH_ISSUE_RESPONSES",
                    self.directory.path().join("issue-responses"),
                )
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
        pull_request_node_with("PR_42", 42, "Shared \"PR\" \u{1b}[31m")
    }

    fn pull_request_node_with(id: &str, number: u64, title: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "number": number,
            "title": title,
            "url": format!("https://github.com/acme/widget/pull/{number}"),
            "repository": {"nameWithOwner": "acme/widget"}
        })
    }

    fn issue_node(id: &str, repository: &str, number: u64, title: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "number": number,
            "title": title,
            "url": format!("https://github.com/{repository}/issues/{number}"),
            "repository": {"nameWithOwner": repository}
        })
    }

    fn issue_response(has_next_page: bool, nodes: serde_json::Value) -> String {
        serde_json::json!({
            "data": {
                "node": {
                    "closingIssuesReferences": {
                        "pageInfo": {"hasNextPage": has_next_page},
                        "nodes": nodes
                    }
                }
            }
        })
        .to_string()
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
        assert_eq!(value["schema_version"], 4);
        assert_eq!(links["repository"], "acme/widget");
        assert_eq!(links["status"], "complete");
        assert_eq!(links["issue_status"], "complete");
        assert_eq!(links["pull_requests"].as_array().unwrap().len(), 1);
        assert_eq!(links["pull_requests"][0]["issue_status"], "complete");
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
        assert_eq!(gh.calls(), 3);
        assert!(gh.log().contains("associatedPullRequests(first: 50)"));
        assert!(gh.log().contains("owner=acme"));
        assert!(gh.log().contains("name=widget"));
        assert!(gh.log().contains("closingIssuesReferences(first: 50)"));
        assert!(!gh.log().contains("userLinkedOnly"));
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
        let human = String::from_utf8_lossy(&human.stdout);
        assert!(human.contains("issue associations (complete)"));
        assert!(!human.contains("closing issues"));
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
        assert_eq!(gh.calls(), 2);
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
        assert_eq!(gh.calls(), 2);
    }

    #[test]
    fn pull_requests_expose_deduplicated_issues_and_preserve_cross_repository_identity() {
        let repo = TestRepo::new();
        commit(&repo, "TwoLayerMarker first", "first");
        commit(&repo, "TwoLayerMarker second", "second");
        repo.index();

        let first_issue = issue_node("I_one", "acme/one", 1, "Issue \"One\" \u{1b}[31m");
        let same_number_elsewhere = issue_node(
            "I_other",
            "acme/two",
            1,
            "Same number, different repository",
        );
        let second_issue = issue_node("I_second", "acme/one", 2, "Second issue");
        let gh = FakeGh::with_responses(
            vec![
                response(
                    false,
                    serde_json::json!([
                        pull_request_node_with("PR_one", 41, "First PR"),
                        pull_request_node_with("PR_two", 42, "Second PR")
                    ]),
                ),
                response(
                    false,
                    serde_json::json!([pull_request_node_with("PR_one", 41, "First PR")]),
                ),
            ],
            vec![
                issue_response(
                    false,
                    serde_json::json!([first_issue.clone(), same_number_elsewhere, second_issue]),
                ),
                issue_response(false, serde_json::json!([first_issue])),
            ],
        );

        let output = gh.run(
            &repo,
            &[
                "search",
                "TwoLayerMarker",
                "--github-links",
                "--github-repo",
                "acme/widget",
                "--json",
            ],
        );

        assert_eq!(output.status.code(), Some(0));
        assert!(!output.stdout.contains(&0x1b));
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let links = &value["github_links"];
        assert_eq!(value["schema_version"], 4);
        assert_eq!(links["issue_status"], "complete");
        assert_eq!(links["issues"].as_array().unwrap().len(), 3);
        assert_eq!(
            links["issues"][0]["title"],
            format!("Issue \"One\" {}[31m", char::from(27))
        );
        assert_eq!(links["issues"][0]["repository"], "acme/one");
        assert_eq!(
            links["issues"][0]["url"],
            "https://github.com/acme/one/issues/1"
        );
        assert_eq!(links["issues"][1]["repository"], "acme/two");
        assert_eq!(links["issues"][1]["number"], 1);
        assert_eq!(links["pull_requests"].as_array().unwrap().len(), 2);
        assert_eq!(links["pull_requests"][0]["issue_status"], "complete");
        assert_eq!(
            links["pull_requests"][0]["issue_urls"],
            serde_json::json!([
                "https://github.com/acme/one/issues/1",
                "https://github.com/acme/two/issues/1",
                "https://github.com/acme/one/issues/2"
            ])
        );
        assert_eq!(
            links["pull_requests"][1]["issue_urls"],
            serde_json::json!(["https://github.com/acme/one/issues/1"])
        );
        assert_eq!(gh.calls(), 4);
    }
}
