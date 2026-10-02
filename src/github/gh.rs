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

const PULL_REQUEST_QUERY: &str = "query($owner: String!, $name: String!, $oid: GitObjectID!, $after: String) { repository(owner: $owner, name: $name) { object(oid: $oid) { ... on Commit { associatedPullRequests(first: 50, after: $after) { nodes { id number title url repository { nameWithOwner } } pageInfo { hasNextPage endCursor } } } } } }";
const ISSUE_QUERY: &str = "query($id: ID!, $after: String) { node(id: $id) { ... on PullRequest { closingIssuesReferences(first: 50, after: $after) { nodes { number title url repository { nameWithOwner } } pageInfo { hasNextPage endCursor } } } } }";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FetchError {
    MissingGh,
    TimedOut,
    RequestFailed,
}

pub(super) fn fetch(
    repository: &GitHubRepository,
    commit_oid: &str,
    cursor: Option<&str>,
    deadline: Instant,
) -> Result<Page, FetchError> {
    let mut variables = vec![
        ("owner", repository.owner.as_str()),
        ("name", repository.name.as_str()),
        ("oid", commit_oid),
    ];
    if let Some(cursor) = cursor {
        variables.push(("after", cursor));
    }
    let (stdout, command_failed) = run_query(PULL_REQUEST_QUERY, &variables, deadline)?;
    parse_response(&stdout, command_failed)
}

pub(super) fn fetch_issues(
    pull_request: &PullRequest,
    cursor: Option<&str>,
    deadline: Instant,
) -> Result<IssuePage, FetchError> {
    let id = pull_request
        .node_id
        .as_deref()
        .ok_or(FetchError::RequestFailed)?;
    let mut variables = vec![("id", id)];
    if let Some(cursor) = cursor {
        variables.push(("after", cursor));
    }
    let (stdout, command_failed) = run_query(ISSUE_QUERY, &variables, deadline)?;
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
    let has_data = response.data.is_some();
    if has_global_api_error(&response.errors, has_data) {
        return Err(FetchError::RequestFailed);
    }
    let connection = response
        .data
        .and_then(|data| data.repository)
        .and_then(|repository| repository.object)
        .and_then(|object| object.associated_pull_requests);
    let Some(connection) = connection else {
        if has_data
            || response
                .errors
                .iter()
                .any(|error| error.get("path").is_some())
        {
            return Ok(Page {
                pull_requests: Vec::new(),
                has_next_page: false,
                end_cursor: None,
                partial: true,
            });
        }
        return Err(FetchError::RequestFailed);
    };

    let mut partial = command_failed || !response.errors.is_empty();
    let page_info = connection.page_info;
    let has_next_page = page_info
        .as_ref()
        .map(|page_info| page_info.has_next_page)
        .unwrap_or_else(|| {
            partial = true;
            true
        });
    let end_cursor = page_info.and_then(|page_info| page_info.end_cursor);
    if has_next_page && end_cursor.is_none() {
        partial = true;
    }
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
        end_cursor,
        partial,
    })
}

fn parse_issue_response(output: &[u8], command_failed: bool) -> Result<IssuePage, FetchError> {
    let response: ApiResponse =
        serde_json::from_slice(output).map_err(|_| FetchError::RequestFailed)?;
    let has_data = response.data.is_some();
    if has_global_api_error(&response.errors, has_data) {
        return Err(FetchError::RequestFailed);
    }
    let connection = response
        .data
        .and_then(|data| data.node)
        .and_then(|node| node.closing_issues_references);
    let Some(connection) = connection else {
        if has_data
            || response
                .errors
                .iter()
                .any(|error| error.get("path").is_some())
        {
            return Ok(IssuePage {
                issues: Vec::new(),
                has_next_page: false,
                end_cursor: None,
                partial: true,
            });
        }
        return Err(FetchError::RequestFailed);
    };

    let mut partial = command_failed || !response.errors.is_empty();
    let page_info = connection.page_info;
    let has_next_page = page_info
        .as_ref()
        .map(|page_info| page_info.has_next_page)
        .unwrap_or_else(|| {
            partial = true;
            true
        });
    let end_cursor = page_info.and_then(|page_info| page_info.end_cursor);
    if has_next_page && end_cursor.is_none() {
        partial = true;
    }
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
        end_cursor,
        partial,
    })
}

fn has_global_api_error(errors: &[Value], has_data: bool) -> bool {
    errors.iter().any(|error| {
        let error_type = error.get("type").and_then(Value::as_str).or_else(|| {
            error
                .get("extensions")
                .and_then(|extensions| extensions.get("type"))
                .and_then(Value::as_str)
        });
        let is_rate_limited =
            error_type.is_some_and(|error_type| error_type.eq_ignore_ascii_case("RATE_LIMITED"));
        let is_authentication_error = error_type.is_some_and(|error_type| {
            error_type.eq_ignore_ascii_case("UNAUTHORIZED")
                || error_type.eq_ignore_ascii_case("UNAUTHENTICATED")
                || error_type.eq_ignore_ascii_case("AUTHENTICATION_ERROR")
                || error_type.eq_ignore_ascii_case("BAD_CREDENTIALS")
                || (error_type.eq_ignore_ascii_case("FORBIDDEN") && error.get("path").is_none())
        }) || error
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|message| message.eq_ignore_ascii_case("Bad credentials"));

        is_rate_limited || is_authentication_error || (!has_data && error.get("path").is_none())
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
    #[serde(rename = "endCursor")]
    end_cursor: Option<String>,
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
        let response = br#"{"data":{"node":null},"errors":[{"type":"FORBIDDEN","message":"Resource not accessible by integration","path":["node","closingIssuesReferences"]}]}"#;
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

    #[test]
    fn both_graphql_connections_expose_the_opaque_end_cursor() {
        let pull_request_response = br#"{"data":{"repository":{"object":{"associatedPullRequests":{"nodes":[],"pageInfo":{"hasNextPage":true,"endCursor":"pr-cursor"}}}}}}"#;
        let pull_request_page = super::parse_response(pull_request_response, false).unwrap();
        assert_eq!(pull_request_page.end_cursor.as_deref(), Some("pr-cursor"));

        let issue_response = br#"{"data":{"node":{"closingIssuesReferences":{"nodes":[],"pageInfo":{"hasNextPage":true,"endCursor":"issue-cursor"}}}}}"#;
        let issue_page = parse_issue_response(issue_response, false).unwrap();
        assert_eq!(issue_page.end_cursor.as_deref(), Some("issue-cursor"));
    }
    #[test]
    fn inaccessible_commit_field_is_partial_instead_of_empty_or_failed() {
        let response = br#"{"data":{"repository":{"object":null}},"errors":[{"message":"Resource not accessible by integration","path":["repository","object"]}]}"#;
        let page = super::parse_response(response, true)
            .expect("partial commit data should not stop later commits");
        assert!(page.partial);
        assert!(page.pull_requests.is_empty());
    }

    #[test]
    fn rate_limit_error_with_partial_data_remains_a_failure() {
        let response = br#"{"data":{"node":{"closingIssuesReferences":{"nodes":[],"pageInfo":{"hasNextPage":false}}}},"errors":[{"type":"RATE_LIMITED","path":["node"]}]}"#;
        assert!(matches!(
            parse_issue_response(response, true),
            Err(FetchError::RequestFailed)
        ));
    }

    #[test]
    fn path_scoped_errors_without_data_remain_local() {
        let pull_request_response = br#"{"data":null,"errors":[{"message":"Not accessible","path":["repository","object"]}]}"#;
        let pull_request_page = super::parse_response(pull_request_response, false)
            .expect("path-scoped errors should be local to this commit");
        assert!(pull_request_page.partial);
        assert!(pull_request_page.pull_requests.is_empty());

        let issue_response = br#"{"data":null,"errors":[{"message":"Not accessible","path":["node","closingIssuesReferences"]}]}"#;
        let issue_page = parse_issue_response(issue_response, false)
            .expect("path-scoped errors should be local to this pull request");
        assert!(issue_page.partial);
        assert!(issue_page.issues.is_empty());
    }

    #[test]
    fn partial_data_with_unscoped_errors_remains_usable() {
        let response = br#"{"data":{"repository":{"object":{"associatedPullRequests":{"nodes":[],"pageInfo":{"hasNextPage":false}}}}},"errors":[{"message":"A field could not be resolved"}]}"#;
        let page = super::parse_response(response, false)
            .expect("partial data without error paths should remain usable");
        assert!(page.partial);
    }

    #[test]
    fn authentication_error_with_partial_data_stops_fetching() {
        let response = br#"{"data":{"node":{"closingIssuesReferences":{"nodes":[],"pageInfo":{"hasNextPage":false}}}},"errors":[{"type":"UNAUTHORIZED","path":["node"]}]}"#;
        assert!(matches!(
            parse_issue_response(response, false),
            Err(FetchError::RequestFailed)
        ));
    }
}
