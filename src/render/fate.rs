use serde::Serialize;

use super::{
    json::{self, JsonPath, JsonSearchScope},
    material,
};
use crate::analysis::capabilities::fate::{Event, Report};

pub(super) fn format_report(report: &Report) -> String {
    let mut lines = vec![format!(
        "Fate of {} line {} at {}",
        super::escape::path(&report.path),
        report.line,
        report.start_revision
    )];
    lines.push(format!("Endpoint: {}", report.endpoint));
    if let Some(scope) = &report.scope {
        lines.push(material::scope_summary(scope));
    }
    lines.push(format!("Final state: {}", report.final_state.code()));
    if let Some(reason) = &report.stop_reason {
        lines.push(format!(
            "Stopped at {} because {}.",
            reason.code, reason.explanation
        ));
    }
    match &report.last_location {
        Some(location) => lines.push(format!(
            "Last confirmed location: {} line {}.",
            super::escape::path(&location.path),
            location.line
        )),
        None => lines.push("Last confirmed location: none.".to_owned()),
    }
    lines.push(format!(
        "Inspected {} forward commit(s){}; {} event(s) recorded{}.",
        report.inspected_commits,
        report
            .max_commits
            .map(|ceiling| format!(" of a {} commit budget", ceiling))
            .unwrap_or_default(),
        if report.traversal_truncated {
            "; traversal truncated by the budget"
        } else {
            ""
        },
        report.total_events,
    ));
    if report.display_truncated {
        lines.push(format!(
            "Showing the last {} of {} events; earlier events are omitted from display only.",
            report.limit, report.total_events
        ));
    }
    for event in &report.events {
        lines.push(format!(
            "{}  {}  {}  {} -> {}",
            event.commit_id,
            event.relationship,
            super::escape::path(&event.before.path),
            event.before.line,
            event
                .after
                .as_ref()
                .map(|after| format!("{} line {}", super::escape::path(&after.path), after.line))
                .unwrap_or_else(|| "deleted".to_owned()),
        ));
        lines.push(format!("    {}", super::escape::subject(&event.subject)));
        lines.push(format!(
            "    Inspect commit/diff with `git show {}`.",
            event.commit_id
        ));
    }
    lines.join("\n")
}

pub(super) fn format_json_report(
    report: &Report,
    additional_warnings: &[String],
    _github_links: Option<&crate::github::LinksReport>,
) -> Result<String, serde_json::Error> {
    let events = report
        .events
        .iter()
        .map(JsonEvent::from)
        .collect::<Vec<_>>();
    let output = JsonReport {
        schema_version: 1,
        kind: "fate",
        start_revision: &report.start_revision,
        endpoint: &report.endpoint,
        path: json::json_path(&report.path),
        line: report.line,
        final_state: report.final_state.code(),
        stopped_at: report.stopped_at.as_deref(),
        stop_reason: report.stop_reason.as_ref().map(|reason| JsonStopReason {
            code: reason.code,
            explanation: reason.explanation,
        }),
        last_location: report.last_location.as_ref().map(|location| JsonLocation {
            path: json::json_path(&location.path),
            line: location.line,
        }),
        inspected_commits: report.inspected_commits,
        max_commits: report.max_commits,
        traversal_truncated: report.traversal_truncated,
        total_events: report.total_events,
        display_truncated: report.display_truncated,
        limit: report.limit,
        events,
        warnings: additional_warnings.iter().collect(),
        notices: Vec::new(),
        scope: report.scope.as_ref().map(json::json_scope),
    };
    serde_json::to_string(&output)
}

#[derive(Serialize)]
struct JsonReport<'a> {
    schema_version: u8,
    kind: &'static str,
    start_revision: &'a str,
    endpoint: &'a str,
    path: JsonPath<'a>,
    line: usize,
    final_state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    stopped_at: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_reason: Option<JsonStopReason<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_location: Option<JsonLocation<'a>>,
    inspected_commits: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_commits: Option<usize>,
    traversal_truncated: bool,
    total_events: usize,
    display_truncated: bool,
    limit: usize,
    events: Vec<JsonEvent<'a>>,
    warnings: Vec<&'a String>,
    notices: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<JsonSearchScope<'a>>,
}

#[derive(Serialize)]
struct JsonStopReason<'a> {
    code: &'a str,
    explanation: &'a str,
}

#[derive(Serialize)]
struct JsonLocation<'a> {
    path: JsonPath<'a>,
    line: i64,
}

#[derive(Serialize)]
struct JsonEvent<'a> {
    commit_id: &'a str,
    timestamp: i64,
    subject: &'a str,
    relationship: &'a str,
    before: JsonLocation<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<JsonLocation<'a>>,
    parent_count: usize,
}

impl<'a> From<&'a Event> for JsonEvent<'a> {
    fn from(event: &'a Event) -> Self {
        Self {
            commit_id: &event.commit_id,
            timestamp: event.commit_time,
            subject: &event.subject,
            relationship: event.relationship,
            before: JsonLocation {
                path: json::json_path(&event.before.path),
                line: event.before.line,
            },
            after: event.after.as_ref().map(|after| JsonLocation {
                path: json::json_path(&after.path),
                line: after.line,
            }),
            parent_count: event.parent_count,
        }
    }
}
