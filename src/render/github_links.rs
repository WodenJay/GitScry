use crate::github::{CommitStatus, FetchStatus, IssueStatus, LinksReport};

use super::escape;

pub(super) fn format(links: &LinksReport) -> String {
    let repository = links
        .repository
        .as_deref()
        .map(escape::subject)
        .unwrap_or_else(|| "repository unresolved".to_owned());
    let status = match links.status {
        FetchStatus::Complete => "complete",
        FetchStatus::Partial => "partial",
        FetchStatus::Failed => "failed",
    };
    let mut lines = vec![format!("GitHub associations ({repository}; {status}):")];

    if let Some(reason) = &links.reason {
        lines.push(format!("  note: {}", escape::subject(reason)));
    }
    if links.pull_requests.is_empty() {
        let message = match links.status {
            FetchStatus::Complete => "  no pull requests were associated",
            FetchStatus::Partial => "  no pull-request links were returned",
            FetchStatus::Failed => "  pull-request associations are unavailable",
        };
        lines.push(message.to_owned());
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
        let status = match association.status {
            CommitStatus::Complete => "complete",
            CommitStatus::Partial => "partial",
            CommitStatus::Failed => "failed",
            CommitStatus::NotQueried => "not queried",
        };
        let urls = association
            .pull_request_urls
            .iter()
            .map(|url| escape::subject(url))
            .collect::<Vec<_>>();
        let related = if urls.is_empty() {
            match association.status {
                CommitStatus::Complete => "none associated".to_owned(),
                CommitStatus::Partial => "association incomplete".to_owned(),
                CommitStatus::Failed => "association unavailable".to_owned(),
                CommitStatus::NotQueried => "not queried".to_owned(),
            }
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
