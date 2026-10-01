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

    let issue_status = match links.issue_status {
        IssueStatus::NotQueried => "not queried",
    };
    lines.push(format!("  issues: {issue_status}"));
    lines.join("\n")
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
