use serde::Serialize;

use super::{
    json::{self, JsonPath, JsonSearchScope},
    material,
};
use crate::analysis::capabilities::fate::{Event, Report};
use crate::git::SymbolSelection;

pub(super) fn format_report(report: &Report) -> String {
    let mut lines = vec![format!(
        "Fate of {} line {} at {}",
        super::escape::path(&report.path),
        report.line,
        report.start_revision
    )];
    if let Some(selection) = &report.symbol_selection {
        lines.push(material::format_symbol_selection(selection));
    }
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
    if !report.associations.is_empty() {
        lines.push("Possible move locations:".to_owned());
        for association in &report.associations {
            let path = super::escape::path(&association.location.path);
            match association.span {
                Some((start, end)) => lines.push(format!("  {path} lines {start}-{end}")),
                None => lines.push(format!("  {path} line {}", association.location.line)),
            }
        }
    }
    if report.associations_truncated {
        lines.push("Additional move locations omitted (candidate limit reached).".to_owned());
    }
    lines.push(format!(
        "Inspected {} forward commit(s){}; {} event(s) recorded{}.",
        report.inspected_commits,
        report
            .max_commits
            .map(|ceiling| format!(" of a {} commit budget", ceiling))
            .unwrap_or_default(),
        report.total_events,
        if report.traversal_truncated {
            "; traversal truncated by the budget"
        } else {
            ""
        },
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
        super::material::render_patch(&mut lines, event.patch.as_ref(), "    ");
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
        schema_version: if report.patch_mode { 2 } else { 1 },
        kind: "fate",
        start_revision: &report.start_revision,
        endpoint: &report.endpoint,
        path: json::json_path(&report.path),
        line: report.line,
        symbol_selection: report.symbol_selection.as_ref(),
        final_state: report.final_state.code(),
        stopped_at: report.stopped_at.as_deref(),
        stop_reason: report.stop_reason.as_ref().map(|reason| JsonStopReason {
            code: reason.code,
            explanation: reason.explanation,
        }),
        associations: report
            .associations
            .iter()
            .map(|association| {
                json_location(
                    &association.location.path,
                    Some(association.location.line),
                    association.span.map(|span| span.0),
                    association.span.map(|span| span.1),
                )
            })
            .collect(),
        associations_truncated: report.associations_truncated,
        last_location: report.last_location.as_ref().map(|location| {
            json_location(
                &location.path,
                Some(location.line),
                report.last_span.map(|span| span.0),
                report.last_span.map(|span| span.1),
            )
        }),
        last_span: report.last_span,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol_selection: Option<&'a SymbolSelection>,
    final_state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    stopped_at: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_reason: Option<JsonStopReason<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    associations: Vec<JsonLocation<'a>>,
    #[serde(skip_serializing_if = "is_false")]
    associations_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_location: Option<JsonLocation<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_span: Option<(i64, i64)>,
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

fn is_false(value: &bool) -> bool {
    !value
}
#[derive(Serialize)]
struct JsonLocation<'a> {
    path: JsonPath<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_line: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    end_line: Option<i64>,
}

fn json_location<'a>(
    path: &'a [u8],
    line: Option<i64>,
    start_line: Option<i64>,
    end_line: Option<i64>,
) -> JsonLocation<'a> {
    JsonLocation {
        path: json::json_path(path),
        line,
        start_line,
        end_line,
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    patch: Option<super::json::JsonPatch<'a>>,
}

impl<'a> From<&'a Event> for JsonEvent<'a> {
    fn from(event: &'a Event) -> Self {
        let (start, end) = event.before_span.unwrap_or((0, 0));
        let after_span = event.after_span;
        Self {
            commit_id: &event.commit_id,
            timestamp: event.commit_time,
            subject: &event.subject,
            relationship: event.relationship,
            before: json_location(
                &event.before.path,
                Some(event.before.line),
                Some(start),
                Some(end),
            ),
            after: event.after.as_ref().map(|after| {
                json_location(
                    &after.path,
                    Some(after.line),
                    after_span.map(|span| span.0),
                    after_span.map(|span| span.1),
                )
            }),
            parent_count: event.parent_count,
            patch: event.patch.as_ref().map(json::json_patch),
        }
    }
}
