use std::{
    io::Read,
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::Deserialize;
use serde_json::Value;

use super::links::{Page, PullRequest};

const QUERY: &str = "query($owner: String!, $name: String!, $oid: GitObjectID!) { repository(owner: $owner, name: $name) { object(oid: $oid) { ... on Commit { associatedPullRequests(first: 50) { nodes { number title url repository { nameWithOwner } } pageInfo { hasNextPage } } } } } }";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FetchError {
    MissingGh,
    TimedOut,
    RequestFailed,
}

pub(super) fn fetch(
    repository: &str,
    commit_oid: &str,
    deadline: Instant,
) -> Result<Page, FetchError> {
    if Instant::now() >= deadline {
        return Err(FetchError::TimedOut);
    }

    let (owner, name) = repository
        .split_once('/')
        .ok_or(FetchError::RequestFailed)?;
    let output = Command::new("gh")
        .args(["api", "graphql", "-f"])
        .arg(format!("query={QUERY}"))
        .args(["-F"])
        .arg(format!("owner={owner}"))
        .args(["-F"])
        .arg(format!("name={name}"))
        .args(["-F"])
        .arg(format!("oid={commit_oid}"))
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
    parse_response(&stdout, !status.success())
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
        pull_requests.push(PullRequest {
            kind: "pull_request",
            number: pull_request.number,
            title: pull_request.title,
            url: pull_request.url,
            repository: repository.name_with_owner,
        });
    }

    Ok(Page {
        pull_requests,
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
}

#[derive(Deserialize)]
struct ApiRepository {
    object: Option<ApiObject>,
}

#[derive(Deserialize)]
struct ApiObject {
    #[serde(rename = "associatedPullRequests")]
    associated_pull_requests: Option<ApiConnection>,
}

#[derive(Deserialize)]
struct ApiConnection {
    nodes: Option<Vec<Option<ApiPullRequest>>>,
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
