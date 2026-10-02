use crate::github::{CommitStatus, FetchStatus, IssueStatus, LinksReport};

use super::escape;

pub(super) fn format(links: &LinksReport) -> String {
    let repository = links
        .repository
        .as_deref()
        .map(escape::subject)
        .unwrap_or_else(|| "repository unresolved".to_owned());
    let (status, empty_message) = fetch_presentation(links.status);
    let mut lines = vec![format!("GitHub associations ({repository}; {status}):")];

    if let Some(reason) = &links.reason {
        lines.push(format!("  note: {}", escape::subject(reason)));
    }
    if links.pull_requests.is_empty() {
        lines.push(empty_message.to_owned());
    } else {
        for pull_request in &links.pull_requests {
            lines.push(format!(
                "  pull request #{} [{}]: {}",
                pull_request.number,
                escape::subject(&pull_request.repository),
                escape::subject(&pull_request.title),
            ));
            lines.push(format!("    {}", escape::subject(&pull_request.url)));
            let (issue_status, empty_message) = issue_presentation(pull_request.issue_status);
            let issue_urls = pull_request
                .issue_urls
                .iter()
                .map(|url| escape::subject(url))
                .collect::<Vec<_>>();
            let related = if issue_urls.is_empty() {
                empty_message.to_owned()
            } else {
                issue_urls.join(", ")
            };
            lines.push(format!(
                "    issue associations ({issue_status}): {related}"
            ));
            if let Some(reason) = &pull_request.issue_reason {
                lines.push(format!("      note: {}", escape::subject(reason)));
            }
        }
    }

    for association in &links.commit_associations {
        let (status, empty_message) = commit_presentation(association.status);
        let urls = association
            .pull_request_urls
            .iter()
            .map(|url| escape::subject(url))
            .collect::<Vec<_>>();
        let related = if urls.is_empty() {
            empty_message.to_owned()
        } else {
            urls.join(", ")
        };
        lines.push(format!(
            "  commit {}: {related} ({status})",
            association.commit_sha
        ));
    }

    let (issue_status, issue_empty_message) = issue_presentation(links.issue_status);
    lines.push(format!("  issue lookup: {issue_status}"));
    if let Some(reason) = &links.issue_reason {
        lines.push(format!("  issue note: {}", escape::subject(reason)));
    }
    if links.issues.is_empty() {
        if links.issue_status == IssueStatus::Complete {
            lines.push(issue_empty_message.to_owned());
        }
    } else {
        lines.push("  associated issues:".to_owned());
        for issue in &links.issues {
            lines.push(format!(
                "    issue #{} [{}]: {}",
                issue.number,
                escape::subject(&issue.repository),
                escape::subject(&issue.title),
            ));
            lines.push(format!("      {}", escape::subject(&issue.url)));
        }
    }
    lines.join("\n")
}

fn issue_presentation(status: IssueStatus) -> (&'static str, &'static str) {
    match status {
        IssueStatus::NotQueried => ("not queried", "    not queried"),
        IssueStatus::Complete => ("complete", "    none associated"),
        IssueStatus::Partial => ("partial", "    lookup incomplete"),
        IssueStatus::Failed => ("failed", "    lookup unavailable"),
    }
}

fn fetch_presentation(status: FetchStatus) -> (&'static str, &'static str) {
    match status {
        FetchStatus::Complete => ("complete", "  no pull requests were associated"),
        FetchStatus::Partial => ("partial", "  no pull-request links were returned"),
        FetchStatus::Failed => ("failed", "  pull-request associations are unavailable"),
    }
}

fn commit_presentation(status: CommitStatus) -> (&'static str, &'static str) {
    match status {
        CommitStatus::Complete => ("complete", "none associated"),
        CommitStatus::Partial => ("partial", "association incomplete"),
        CommitStatus::Failed => ("failed", "association unavailable"),
        CommitStatus::NotQueried => ("not queried", "not queried"),
    }
}
