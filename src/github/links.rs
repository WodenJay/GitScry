use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use serde::Serialize;

use crate::analysis::{Report, ReportKind};

use super::{gh, remote};

const MAX_API_REQUESTS: usize = 20;
const MAX_LINKED_OBJECTS: usize = 200;
const ASSOCIATION_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Serialize)]
pub(crate) struct LinksReport {
    pub(crate) repository: Option<String>,
    pub(crate) status: FetchStatus,
    pub(crate) reason: Option<String>,
    pub(crate) issue_status: IssueStatus,
    pub(crate) issue_reason: Option<String>,
    pub(crate) pull_requests: Vec<PullRequest>,
    pub(crate) issues: Vec<Issue>,
    pub(crate) commit_associations: Vec<CommitAssociation>,
}

impl LinksReport {
    fn new(
        repository: Option<String>,
        status: FetchStatus,
        reason: Option<String>,
        commit_associations: Vec<CommitAssociation>,
    ) -> Self {
        Self {
            repository,
            status,
            reason,
            issue_status: IssueStatus::NotQueried,
            issue_reason: None,
            pull_requests: Vec::new(),
            issues: Vec::new(),
            commit_associations,
        }
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FetchStatus {
    Complete,
    Partial,
    Failed,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CommitStatus {
    Complete,
    Partial,
    Failed,
    NotQueried,
}

#[derive(Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IssueStatus {
    NotQueried,
    Complete,
    Partial,
    Failed,
}

#[derive(Serialize)]
pub(crate) struct PullRequest {
    #[serde(rename = "type")]
    pub(super) kind: &'static str,
    #[serde(skip)]
    pub(super) node_id: Option<String>,
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) url: String,
    pub(crate) repository: String,
    pub(crate) issue_status: IssueStatus,
    pub(crate) issue_reason: Option<String>,
    pub(crate) issue_urls: Vec<String>,
}

#[derive(Serialize)]
pub(crate) struct Issue {
    #[serde(rename = "type")]
    pub(super) kind: &'static str,
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) url: String,
    pub(crate) repository: String,
}

#[derive(Serialize)]
pub(crate) struct CommitAssociation {
    pub(crate) commit_sha: String,
    pub(crate) status: CommitStatus,
    pub(crate) pull_request_urls: Vec<String>,
}

pub(super) struct Page {
    pub(super) pull_requests: Vec<PullRequest>,
    pub(super) has_next_page: bool,
    pub(super) partial: bool,
}

pub(super) struct IssuePage {
    pub(super) issues: Vec<Issue>,
    pub(super) has_next_page: bool,
    pub(super) partial: bool,
}

pub(crate) fn fetch(report: &Report, explicit_repository: Option<&str>) -> LinksReport {
    debug_assert!(matches!(report.kind, ReportKind::Search));
    let commits = returned_commits(report);
    if commits.is_empty() {
        return LinksReport::new(None, FetchStatus::Complete, None, Vec::new());
    }

    let deadline = Instant::now() + ASSOCIATION_TIMEOUT;
    let repository = match remote::resolve(explicit_repository) {
        Ok(repository) => repository,
        Err(reason) => return failed_links(None, &commits, reason),
    };
    fetch_pages(&commits, repository, deadline, gh::fetch, gh::fetch_issues)
}

fn returned_commits(report: &Report) -> Vec<String> {
    let mut seen = HashSet::new();
    report
        .materials
        .iter()
        .flat_map(|material| material.citations.iter())
        .filter(|citation| seen.insert(citation.oid.clone()))
        .map(|citation| citation.oid.clone())
        .collect()
}

fn failed_links(repository: Option<String>, commits: &[String], reason: &str) -> LinksReport {
    let commit_associations = commits
        .iter()
        .map(|commit_sha| CommitAssociation {
            commit_sha: commit_sha.clone(),
            status: CommitStatus::Failed,
            pull_request_urls: Vec::new(),
        })
        .collect();
    LinksReport::new(
        repository,
        FetchStatus::Failed,
        Some(reason.to_owned()),
        commit_associations,
    )
}

fn fetch_pages(
    commits: &[String],
    repository: remote::GitHubRepository,
    deadline: Instant,
    mut fetch_page: impl FnMut(&remote::GitHubRepository, &str, Instant) -> Result<Page, gh::FetchError>,
    mut fetch_issue_page: impl FnMut(&PullRequest, Instant) -> Result<IssuePage, gh::FetchError>,
) -> LinksReport {
    let mut result = LinksReport::new(
        Some(repository.name_with_owner()),
        FetchStatus::Complete,
        None,
        commits
            .iter()
            .map(|commit_sha| CommitAssociation {
                commit_sha: commit_sha.clone(),
                status: CommitStatus::NotQueried,
                pull_request_urls: Vec::new(),
            })
            .collect(),
    );
    let mut api_requests = 0;
    let mut global_fetch_failed = false;
    let mut pull_request_indexes = HashMap::<(String, u64), usize>::new();

    for (commit_index, commit_sha) in commits.iter().enumerate() {
        if api_requests == MAX_API_REQUESTS {
            set_reason(
                &mut result.reason,
                "GitHub request limit reached; remaining commits were not queried.",
            );
            break;
        }
        if Instant::now() >= deadline {
            set_reason(
                &mut result.reason,
                "GitHub association lookup timed out; remaining commits were not queried.",
            );
            break;
        }

        api_requests += 1;
        let page = match fetch_page(&repository, commit_sha, deadline) {
            Ok(page) => page,
            Err(error) => {
                result.commit_associations[commit_index].status = CommitStatus::Failed;
                set_reason(&mut result.reason, fetch_error_message(error));
                global_fetch_failed = true;
                break;
            }
        };

        let mut page_limited = false;
        for pull_request in page.pull_requests {
            let key = (
                pull_request.repository.to_ascii_lowercase(),
                pull_request.number,
            );
            let index = if let Some(index) = pull_request_indexes.get(&key) {
                *index
            } else {
                if result.pull_requests.len() + result.issues.len() == MAX_LINKED_OBJECTS {
                    page_limited = true;
                    break;
                }
                let index = result.pull_requests.len();
                pull_request_indexes.insert(key, index);
                result.pull_requests.push(pull_request);
                index
            };
            let url = result.pull_requests[index].url.clone();
            let associations = &mut result.commit_associations[commit_index].pull_request_urls;
            if !associations.contains(&url) {
                associations.push(url);
            }
        }

        let association = &mut result.commit_associations[commit_index];
        association.status = if page.partial || page.has_next_page || page_limited {
            CommitStatus::Partial
        } else {
            CommitStatus::Complete
        };

        if page.partial {
            set_reason(
                &mut result.reason,
                "GitHub returned partial data; some pull-request links may be unavailable.",
            );
        } else if page.has_next_page {
            set_reason(
                &mut result.reason,
                "Some commits have more than 50 associated pull requests; later pages were not fetched.",
            );
        }

        if page_limited {
            set_reason(
                &mut result.reason,
                "GitHub object limit reached; remaining associations were not queried.",
            );
            break;
        }
        if result.pull_requests.len() + result.issues.len() == MAX_LINKED_OBJECTS
            && commit_index + 1 < commits.len()
        {
            set_reason(
                &mut result.reason,
                "GitHub object limit reached; remaining commits were not queried.",
            );
            break;
        }
    }

    result.status = overall_status(&result.commit_associations);
    if !global_fetch_failed {
        fetch_issue_pages(
            &mut result,
            &mut api_requests,
            deadline,
            &mut fetch_issue_page,
        );
    }
    result.issue_status = overall_issue_status(&result.pull_requests);
    result
}

fn fetch_issue_pages(
    result: &mut LinksReport,
    api_requests: &mut usize,
    deadline: Instant,
    fetch_issue_page: &mut impl FnMut(&PullRequest, Instant) -> Result<IssuePage, gh::FetchError>,
) {
    let mut issue_indexes = HashMap::<(String, u64), usize>::new();
    for pull_request_index in 0..result.pull_requests.len() {
        if *api_requests == MAX_API_REQUESTS {
            set_reason(
                &mut result.issue_reason,
                "GitHub request limit reached; remaining issue associations were not queried.",
            );
            break;
        }
        if Instant::now() >= deadline {
            set_reason(
                &mut result.issue_reason,
                "GitHub association lookup timed out; remaining issue associations were not queried.",
            );
            break;
        }
        if result.pull_requests.len() + result.issues.len() == MAX_LINKED_OBJECTS {
            set_reason(
                &mut result.issue_reason,
                "GitHub object limit reached; remaining issue associations were not queried.",
            );
            break;
        }
        if result.pull_requests[pull_request_index].node_id.is_none() {
            let pull_request = &mut result.pull_requests[pull_request_index];
            pull_request.issue_status = IssueStatus::Failed;
            let reason =
                "Pull request ID was unavailable; issue associations could not be queried.";
            pull_request.issue_reason = Some(reason.to_owned());
            set_reason(&mut result.issue_reason, reason);
            continue;
        }

        *api_requests += 1;
        let page = match fetch_issue_page(&result.pull_requests[pull_request_index], deadline) {
            Ok(page) => page,
            Err(error) => {
                let reason = fetch_error_message(error);
                let pull_request = &mut result.pull_requests[pull_request_index];
                pull_request.issue_status = IssueStatus::Failed;
                pull_request.issue_reason = Some(reason.to_owned());
                set_reason(&mut result.issue_reason, reason);
                break;
            }
        };

        let mut object_limited = false;
        for issue in page.issues {
            let key = (issue.repository.to_ascii_lowercase(), issue.number);
            let index = if let Some(index) = issue_indexes.get(&key) {
                *index
            } else {
                if result.pull_requests.len() + result.issues.len() == MAX_LINKED_OBJECTS {
                    object_limited = true;
                    break;
                }
                let index = result.issues.len();
                issue_indexes.insert(key, index);
                result.issues.push(issue);
                index
            };
            let url = result.issues[index].url.clone();
            let issue_urls = &mut result.pull_requests[pull_request_index].issue_urls;
            if !issue_urls.contains(&url) {
                issue_urls.push(url);
            }
        }

        let pull_request = &mut result.pull_requests[pull_request_index];
        if object_limited {
            pull_request.issue_status = IssueStatus::Partial;
            let reason =
                "GitHub object limit reached; remaining issue associations were not queried.";
            pull_request.issue_reason = Some(reason.to_owned());
            set_reason(&mut result.issue_reason, reason);
            break;
        }
        if page.partial || page.has_next_page {
            pull_request.issue_status = IssueStatus::Partial;
            let reason = if page.partial {
                "GitHub returned partial issue data; some issue links may be unavailable."
            } else {
                "Some pull requests have more than 50 associated issues; later pages were not fetched."
            };
            pull_request.issue_reason = Some(reason.to_owned());
            set_reason(&mut result.issue_reason, reason);
        } else {
            pull_request.issue_status = IssueStatus::Complete;
        }
    }
}

fn overall_issue_status(pull_requests: &[PullRequest]) -> IssueStatus {
    if pull_requests.is_empty() {
        return IssueStatus::NotQueried;
    }
    if pull_requests
        .iter()
        .all(|pull_request| matches!(pull_request.issue_status, IssueStatus::Complete))
    {
        IssueStatus::Complete
    } else if pull_requests.iter().any(|pull_request| {
        matches!(
            pull_request.issue_status,
            IssueStatus::Complete | IssueStatus::Partial
        )
    }) {
        IssueStatus::Partial
    } else if pull_requests
        .iter()
        .any(|pull_request| matches!(pull_request.issue_status, IssueStatus::Failed))
    {
        IssueStatus::Failed
    } else {
        IssueStatus::NotQueried
    }
}

fn overall_status(associations: &[CommitAssociation]) -> FetchStatus {
    if associations
        .iter()
        .all(|association| matches!(association.status, CommitStatus::Complete))
    {
        FetchStatus::Complete
    } else if associations.iter().any(|association| {
        matches!(
            association.status,
            CommitStatus::Complete | CommitStatus::Partial
        )
    }) {
        FetchStatus::Partial
    } else {
        FetchStatus::Failed
    }
}

fn set_reason(current: &mut Option<String>, reason: &str) {
    if current.is_none() {
        *current = Some(reason.to_owned());
    }
}

fn fetch_error_message(error: gh::FetchError) -> &'static str {
    match error {
        gh::FetchError::MissingGh => "GitHub CLI 'gh' was not found.",
        gh::FetchError::TimedOut => {
            "GitHub association lookup timed out; remaining commits were not queried."
        }
        gh::FetchError::RequestFailed => {
            "GitHub association lookup failed; check gh authentication, network, or API availability."
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crate::analysis::{self, Citation, Confidence, Material, ReportKind};
    use crate::github::remote::GitHubRepository;

    use super::{
        CommitStatus, FetchStatus, Issue, IssuePage, IssueStatus, MAX_API_REQUESTS,
        MAX_LINKED_OBJECTS, Page, PullRequest, fetch, fetch_pages, returned_commits,
    };

    fn report(citations: &[&[&str]]) -> analysis::Report {
        let materials = citations
            .iter()
            .map(|citations| Material {
                subject: "subject".to_owned(),
                paths: Vec::new(),
                confidence: Confidence::High,
                basis: Vec::new(),
                citations: citations
                    .iter()
                    .map(|oid| Citation::new((*oid).to_owned(), "subject".to_owned()))
                    .collect(),
                detail: None,
                patch: None,
            })
            .collect();
        analysis::report(ReportKind::Search, materials, 0, 1)
    }

    fn repository() -> GitHubRepository {
        GitHubRepository {
            owner: "acme".to_owned(),
            name: "widget".to_owned(),
        }
    }

    fn pull_request(number: u64) -> PullRequest {
        PullRequest {
            kind: "pull_request",
            node_id: Some(format!("PR_{number}")),
            number,
            title: format!("PR {number}"),
            url: format!("https://github.com/acme/widget/pull/{number}"),
            repository: "acme/widget".to_owned(),
            issue_status: super::IssueStatus::NotQueried,
            issue_reason: None,
            issue_urls: Vec::new(),
        }
    }

    fn empty_issue_page() -> IssuePage {
        IssuePage {
            issues: Vec::new(),
            has_next_page: false,
            partial: false,
        }
    }

    fn issue_page(issues: Vec<Issue>) -> IssuePage {
        IssuePage {
            issues,
            has_next_page: false,
            partial: false,
        }
    }

    fn issue(repository: &str, number: u64) -> Issue {
        Issue {
            kind: "issue",
            number,
            title: format!("Issue {number}"),
            url: format!("https://github.com/{repository}/issues/{number}"),
            repository: repository.to_owned(),
        }
    }

    fn page(pull_requests: Vec<PullRequest>) -> Page {
        Page {
            pull_requests,
            has_next_page: false,
            partial: false,
        }
    }

    #[test]
    fn returned_commit_ids_include_all_citations_once_in_first_seen_order() {
        let report = report(&[&["first", "related"], &["related", "last"]]);
        assert_eq!(
            returned_commits(&report),
            ["first", "related", "last"].map(str::to_owned)
        );
    }

    #[test]
    fn empty_search_does_not_resolve_a_repository_or_send_a_request() {
        let report = report(&[]);
        let links = fetch(&report, None);
        assert!(links.commit_associations.is_empty());
        assert!(links.pull_requests.is_empty());
        assert!(matches!(links.status, FetchStatus::Complete));
        assert!(links.repository.is_none());
    }

    #[test]
    fn deduplicates_pull_request_objects_but_keeps_each_commit_relation() {
        let commits = ["first".to_owned(), "second".to_owned()];
        let mut seen = Vec::new();
        let links = fetch_pages(
            &commits,
            repository(),
            Instant::now() + Duration::from_secs(1),
            |_, sha, _| {
                seen.push(sha.to_owned());
                Ok(page(if sha == "first" {
                    vec![pull_request(42), pull_request(43)]
                } else {
                    vec![pull_request(42)]
                }))
            },
            |_, _| Ok(empty_issue_page()),
        );

        assert_eq!(seen, ["first", "second"]);
        assert_eq!(links.pull_requests.len(), 2);
        assert_eq!(links.commit_associations[0].pull_request_urls.len(), 2);
        assert_eq!(links.commit_associations[1].pull_request_urls.len(), 1);
        assert!(matches!(links.status, FetchStatus::Complete));
    }

    #[test]
    fn enforces_request_and_object_budgets_without_dropping_completed_results() {
        let commits = (0..MAX_API_REQUESTS + 1)
            .map(|index| format!("commit-{index}"))
            .collect::<Vec<_>>();
        let mut calls = 0;
        let links = fetch_pages(
            &commits,
            repository(),
            Instant::now() + Duration::from_secs(1),
            |_, _, _| {
                calls += 1;
                Ok(page(Vec::new()))
            },
            |_, _| Ok(empty_issue_page()),
        );
        assert_eq!(calls, MAX_API_REQUESTS);
        assert!(matches!(
            links.commit_associations[MAX_API_REQUESTS].status,
            CommitStatus::NotQueried
        ));
        assert!(matches!(links.status, FetchStatus::Partial));

        let commits = (0..5)
            .map(|index| format!("commit-{index}"))
            .collect::<Vec<_>>();
        let mut page_index = 0;
        let links = fetch_pages(
            &commits,
            repository(),
            Instant::now() + Duration::from_secs(1),
            |_, _, _| {
                let page_number = page_index;
                page_index += 1;
                Ok(page(
                    (0..50)
                        .map(|offset| pull_request((page_number * 50 + offset + 1) as u64))
                        .collect(),
                ))
            },
            |_, _| Ok(empty_issue_page()),
        );
        assert_eq!(page_index, 4);
        assert_eq!(links.pull_requests.len(), MAX_LINKED_OBJECTS);
        assert!(matches!(links.status, FetchStatus::Partial));
    }

    #[test]
    fn unexhausted_first_page_is_marked_partial() {
        let commits = ["first".to_owned()];
        let links = fetch_pages(
            &commits,
            repository(),
            Instant::now() + Duration::from_secs(1),
            |_, _, _| {
                Ok(Page {
                    pull_requests: vec![pull_request(42)],
                    has_next_page: true,
                    partial: false,
                })
            },
            |_, _| Ok(empty_issue_page()),
        );

        assert!(matches!(links.status, FetchStatus::Partial));
        assert!(matches!(
            links.commit_associations[0].status,
            CommitStatus::Partial
        ));
        assert!(
            links
                .reason
                .as_deref()
                .unwrap()
                .contains("later pages were not fetched")
        );
    }
    #[test]
    fn closing_issues_are_deduplicated_by_repository_and_linked_per_pull_request() {
        let commits = ["commit".to_owned()];
        let mut issue_calls = Vec::new();
        let links = fetch_pages(
            &commits,
            repository(),
            Instant::now() + Duration::from_secs(1),
            |_, _, _| Ok(page(vec![pull_request(42), pull_request(43)])),
            |pull_request, _| {
                issue_calls.push(pull_request.number);
                if pull_request.number == 42 {
                    Ok(issue_page(vec![issue("acme/one", 1), issue("acme/two", 1)]))
                } else {
                    Ok(issue_page(vec![issue("acme/one", 1), issue("acme/one", 2)]))
                }
            },
        );

        assert_eq!(issue_calls, [42, 43]);
        assert_eq!(links.issues.len(), 3);
        assert_eq!(
            links
                .issues
                .iter()
                .map(|issue| (issue.repository.clone(), issue.number))
                .collect::<Vec<_>>(),
            [
                ("acme/one".to_owned(), 1),
                ("acme/two".to_owned(), 1),
                ("acme/one".to_owned(), 2)
            ]
        );
        assert_eq!(links.pull_requests[0].issue_urls.len(), 2);
        assert_eq!(links.pull_requests[1].issue_urls.len(), 2);
        assert!(matches!(links.issue_status, IssueStatus::Complete));
        assert!(matches!(
            links.pull_requests[0].issue_status,
            IssueStatus::Complete
        ));
        assert!(matches!(
            links.pull_requests[1].issue_status,
            IssueStatus::Complete
        ));
    }

    #[test]
    fn issue_queries_share_the_api_request_budget_with_pull_request_queries() {
        let commits = (0..MAX_API_REQUESTS - 1)
            .map(|index| format!("commit-{index}"))
            .collect::<Vec<_>>();
        let mut pull_request_calls = 0;
        let mut issue_calls = 0;
        let links = fetch_pages(
            &commits,
            repository(),
            Instant::now() + Duration::from_secs(1),
            |_, _, _| {
                pull_request_calls += 1;
                Ok(page(vec![pull_request(pull_request_calls as u64)]))
            },
            |_, _| {
                issue_calls += 1;
                Ok(empty_issue_page())
            },
        );

        assert_eq!(pull_request_calls + issue_calls, MAX_API_REQUESTS);
        assert_eq!(issue_calls, 1);
        assert!(matches!(
            links.pull_requests[0].issue_status,
            IssueStatus::Complete
        ));
        assert!(matches!(
            links.pull_requests[1].issue_status,
            IssueStatus::NotQueried
        ));
        assert!(matches!(links.issue_status, IssueStatus::Partial));
        assert!(
            links
                .issue_reason
                .as_deref()
                .unwrap()
                .contains("request limit")
        );
    }

    #[test]
    fn issue_page_and_request_failures_are_reported_per_pull_request() {
        let commits = ["commit".to_owned()];
        let links = fetch_pages(
            &commits,
            repository(),
            Instant::now() + Duration::from_secs(1),
            |_, _, _| Ok(page(vec![pull_request(42), pull_request(43)])),
            |pull_request, _| {
                if pull_request.number == 42 {
                    Ok(IssuePage {
                        issues: vec![issue("acme/widget", 1)],
                        has_next_page: true,
                        partial: false,
                    })
                } else {
                    Err(crate::github::gh::FetchError::RequestFailed)
                }
            },
        );

        assert!(matches!(
            links.pull_requests[0].issue_status,
            IssueStatus::Partial
        ));
        assert_eq!(links.pull_requests[0].issue_urls.len(), 1);
        assert!(matches!(
            links.pull_requests[1].issue_status,
            IssueStatus::Failed
        ));
        assert!(links.pull_requests[1].issue_reason.is_some());
        assert!(matches!(links.issue_status, IssueStatus::Partial));
        assert!(
            links
                .issue_reason
                .as_deref()
                .unwrap()
                .contains("later pages")
        );
        assert!(matches!(links.status, FetchStatus::Complete));
    }
    #[test]
    fn no_pull_requests_means_issue_layer_was_not_queried() {
        let commits = ["commit".to_owned()];
        let links = fetch_pages(
            &commits,
            repository(),
            Instant::now() + Duration::from_secs(1),
            |_, _, _| Ok(page(Vec::new())),
            |_, _| panic!("issue lookup must not run without pull requests"),
        );

        assert!(matches!(links.issue_status, IssueStatus::NotQueried));
    }

    #[test]
    fn partial_issue_lookup_does_not_prevent_later_pull_requests() {
        let commits = ["commit".to_owned()];
        let mut issue_calls = Vec::new();
        let links = fetch_pages(
            &commits,
            repository(),
            Instant::now() + Duration::from_secs(1),
            |_, _, _| Ok(page(vec![pull_request(42), pull_request(43)])),
            |pull_request, _| {
                issue_calls.push(pull_request.number);
                if pull_request.number == 42 {
                    Ok(IssuePage {
                        issues: Vec::new(),
                        has_next_page: false,
                        partial: true,
                    })
                } else {
                    Ok(issue_page(vec![issue("acme/widget", 1)]))
                }
            },
        );

        assert_eq!(issue_calls, [42, 43]);
        assert!(matches!(
            links.pull_requests[0].issue_status,
            IssueStatus::Partial
        ));
        assert!(matches!(
            links.pull_requests[1].issue_status,
            IssueStatus::Complete
        ));
        assert!(matches!(links.issue_status, IssueStatus::Partial));
        assert_eq!(links.pull_requests[1].issue_urls.len(), 1);
    }
}
