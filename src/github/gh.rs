use std::{
    io::Read,
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::Deserialize;
use serde_json::Value;

use super::{
    links::{Issue, IssuePage, Page, PullRequest},
    remote::GitHubRepository,
};

const PULL_REQUEST_QUERY: &str = "query($owner: String!, $name: String!, $oid: GitObjectID!) { repository(owner: $owner, name: $name) { object(oid: $oid) { ... on Commit { associatedPullRequests(first: 50) { nodes { id number title url repository { nameWithOwner } } pageInfo { hasNextPage } } } } } }";
const ISSUE_QUERY: &str = "query($id: ID!) { node(id: $id) { ... on PullRequest { closingIssuesReferences(first: 50) { nodes { number title url repository { nameWithOwner } } pageInfo { hasNextPage } } } } }";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FetchError {
    MissingGh,
    TimedOut,
    RequestFailed,
}

pub(super) fn fetch(
    repository: &GitHubRepository,
    commit_oid: &str,
    deadline: Instant,
) -> Result<Page, FetchError> {
    let (stdout, command_failed) = run_query(
        PULL_REQUEST_QUERY,
        &[
            ("owner", &repository.owner),
            ("name", &repository.name),
            ("oid", commit_oid),
        ],
        deadline,
    )?;
    parse_response(&stdout, command_failed)
}

pub(super) fn fetch_issues(
    pull_request: &PullRequest,
    deadline: Instant,
) -> Result<IssuePage, FetchError> {
    let id = pull_request
        .node_id
        .as_deref()
        .ok_or(FetchError::RequestFailed)?;
    let (stdout, command_failed) = run_query(ISSUE_QUERY, &[("id", id)], deadline)?;
    parse_issue_response(&stdout, command_failed)
}

fn run_query(
    query: &str,
    variables: &[(&str, &str)],
    deadline: Instant,
) -> Result<(Vec<u8>, bool), FetchError> {
    if Instant::now() >= deadline {
        return Err(FetchError::TimedOut);
    }

    let mut command = Command::new("gh");
    command.args(["api", "graphql", "-f"]);
    command.arg(format!("query={query}"));
    for (name, value) in variables {
        command.args(["-F"]).arg(format!("{name}={value}"));
    }
    let output = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                FetchError::MissingGh
            } else {
                FetchError::RequestFailed
            }
        })?;
    let (status, stdout) = wait_for_output(output, deadline)?;
    Ok((stdout, !status.success()))
}

fn wait_for_output(
    mut child: std::process::Child,
    deadline: Instant,
) -> Result<(ExitStatus, Vec<u8>), FetchError> {
    let stdout = child.stdout.take().ok_or(FetchError::RequestFailed)?;
    let stderr = child.stderr.take().ok_or(FetchError::RequestFailed)?;
    let stdout_reader = thread::spawn(move || read_all(stdout));
    let stderr_reader = thread::spawn(move || discard(stderr));

    let status = loop {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(FetchError::TimedOut);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(FetchError::RequestFailed);
            }
        }
    };

    let stdout = stdout_reader
        .join()
        .map_err(|_| FetchError::RequestFailed)?;
    let _ = stderr_reader.join();
    Ok((status, stdout))
}

fn read_all(mut reader: impl Read) -> Vec<u8> {
    let mut output = Vec::new();
    let _ = reader.read_to_end(&mut output);
    output
}

fn discard(mut reader: impl Read) {
    let _ = std::io::copy(&mut reader, &mut std::io::sink());
}

fn parse_response(output: &[u8], command_failed: bool) -> Result<Page, FetchError> {
    let response: ApiResponse =
        serde_json::from_slice(output).map_err(|_| FetchError::RequestFailed)?;
    let connection = response
        .data
        .and_then(|data| data.repository)
        .and_then(|repository| repository.object)
        .and_then(|object| object.associated_pull_requests)
        .ok_or(FetchError::RequestFailed)?;

    let mut partial = command_failed || !response.errors.is_empty();
    let has_next_page = connection
        .page_info
        .map(|page_info| page_info.has_next_page)
        .unwrap_or_else(|| {
            partial = true;
            true
        });
    let mut pull_requests = Vec::new();
    for pull_request in connection.nodes.unwrap_or_else(|| {
        partial = true;
        Vec::new()
    }) {
        let Some(pull_request) = pull_request else {
            partial = true;
            continue;
        };
        let Some(repository) = pull_request.repository else {
            partial = true;
            continue;
        };
        if pull_request.id.is_none() {
            partial = true;
        }
        pull_requests.push(PullRequest {
            kind: "pull_request",
            node_id: pull_request.id,
            number: pull_request.number,
            title: pull_request.title,
            url: pull_request.url,
            repository: repository.name_with_owner,
            issue_status: super::links::IssueStatus::NotQueried,
            issue_reason: None,
            issue_urls: Vec::new(),
        });
    }

    Ok(Page {
        pull_requests,
        has_next_page,
        partial,
    })
}

fn parse_issue_response(output: &[u8], command_failed: bool) -> Result<IssuePage, FetchError> {
    let response: ApiResponse =
        serde_json::from_slice(output).map_err(|_| FetchError::RequestFailed)?;
    let has_data = response.data.is_some();
    let connection = response
        .data
        .and_then(|data| data.node)
        .and_then(|node| node.closing_issues_references);
    let Some(connection) = connection else {
        if has_data {
            return Ok(IssuePage {
                issues: Vec::new(),
                has_next_page: false,
                partial: true,
            });
        }
        return Err(FetchError::RequestFailed);
    };

    let mut partial = command_failed || !response.errors.is_empty();
    let has_next_page = connection
        .page_info
        .map(|page_info| page_info.has_next_page)
        .unwrap_or_else(|| {
            partial = true;
            true
        });
    let mut issues = Vec::new();
    for issue in connection.nodes.unwrap_or_else(|| {
        partial = true;
        Vec::new()
    }) {
        let Some(issue) = issue else {
            partial = true;
            continue;
        };
        let Some(repository) = issue.repository else {
            partial = true;
            continue;
        };
        issues.push(Issue {
            kind: "issue",
            number: issue.number,
            title: issue.title,
            url: issue.url,
            repository: repository.name_with_owner,
        });
    }

    Ok(IssuePage {
        issues,
        has_next_page,
        partial,
    })
}

#[derive(Deserialize)]
struct ApiResponse {
    data: Option<ApiData>,
    #[serde(default)]
    errors: Vec<Value>,
}

#[derive(Deserialize)]
struct ApiData {
    repository: Option<ApiRepository>,
    node: Option<ApiNode>,
}

#[derive(Deserialize)]
struct ApiRepository {
    object: Option<ApiObject>,
}

#[derive(Deserialize)]
struct ApiObject {
    #[serde(rename = "associatedPullRequests")]
    associated_pull_requests: Option<ApiConnection<ApiPullRequest>>,
}

#[derive(Deserialize)]
struct ApiNode {
    #[serde(rename = "closingIssuesReferences")]
    closing_issues_references: Option<ApiConnection<ApiIssue>>,
}

#[derive(Deserialize)]
struct ApiConnection<T> {
    nodes: Option<Vec<Option<T>>>,
    #[serde(rename = "pageInfo")]
    page_info: Option<ApiPageInfo>,
}

#[derive(Deserialize)]
struct ApiPageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
}

#[derive(Deserialize)]
struct ApiPullRequest {
    id: Option<String>,
    number: u64,
    title: String,
    url: String,
    repository: Option<ApiPullRequestRepository>,
}

#[derive(Deserialize)]
struct ApiIssue {
    number: u64,
    title: String,
    url: String,
    repository: Option<ApiPullRequestRepository>,
}

#[derive(Deserialize)]
struct ApiPullRequestRepository {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };

    use super::{FetchError, wait_for_output};

    #[test]
    fn kills_a_child_that_outlives_its_deadline() {
        let child = Command::new("sh")
            .args(["-c", "exec sleep 10"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let start = Instant::now();
        let result = wait_for_output(child, start + Duration::from_millis(50));
        assert!(matches!(result, Err(FetchError::TimedOut)));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}

#[cfg(test)]
mod issue_response_tests {
    use super::{FetchError, parse_issue_response};

    #[test]
    fn inaccessible_issue_field_is_partial_instead_of_empty_or_failed() {
        let response = br#"{"data":{"node":null},"errors":[{"message":"Resource not accessible by integration","path":["node","closingIssuesReferences"]}]}"#;
        let page = parse_issue_response(response, true)
            .expect("partial issue data should not stop later pull requests");

        assert!(page.partial);
        assert!(page.issues.is_empty());
    }

    #[test]
    fn global_error_without_data_remains_a_failure() {
        let response = br#"{"errors":[{"message":"Bad credentials"}]}"#;

        assert!(matches!(
            parse_issue_response(response, true),
            Err(FetchError::RequestFailed)
        ));
    }
}
