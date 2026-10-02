use serde::Serialize;

use super::{
    escape,
    json::{self, JsonPath, JsonSearchScope},
    material,
};
use crate::analysis::TraceRemovalReport as Report;

const SAME_COMMIT_NAVIGATION_NOTICE: &str = "Same-commit co-changes are navigation only; they do not prove replacement, migration intent, causality, or cross-file identity.";
pub(super) fn format_report(report: &Report) -> String {
    let mut lines = vec![format!(
        "Deletion events for literal {} in published cache {}",
        escape::code_line(report.query.as_bytes()),
        report.cache_tip
    )];
    if let Some(path) = &report.path {
        lines.push(format!("Exact historical old path: {}", escape::path(path)));
    }
    if let Some(scope) = &report.scope {
        lines.push(material::scope_summary(scope));
    }
    if report.events.is_empty() {
        lines.push("No matching deletion events found within the query scope.".to_owned());
    } else {
        lines.push(format!(
            "Showing {} of {} matching deletion events (limit {}; truncated: {}).",
            report.events.len(),
            report.matched_count,
            report.limit,
            report.truncated
        ));
        for event in &report.events {
            lines.push(format!(
                "{}  {}  {}  {}",
                event.timestamp,
                event.commit_id,
                escape::subject(&event.status),
                escape::path(&event.old_path)
            ));
            if let Some(path) = &event.new_path {
                lines.push(format!("    Detected new path: {}", escape::path(path)));
            }
            lines.push("    Complete commit message:".to_owned());
            for line in event.message.split(|byte| *byte == b'\n') {
                lines.push(format!("        {}", escape::code_line(line)));
            }
            lines.push(format!("    First parent: {}", event.first_parent_id));
            lines.push(format!(
                "    Preceding file locator: {}:{}",
                event.first_parent_id,
                escape::path(&event.old_path)
            ));
            for matched in &event.matches {
                lines.push(format!(
                    "    -{}: {}",
                    matched.line_number,
                    escape::code_line(&matched.line)
                ));
            }

            lines.push(format!(
                "    Same-commit file navigation: {}",
                event.same_commit_files.status.as_str()
            ));
            lines.push(format!("      {SAME_COMMIT_NAVIGATION_NOTICE}"));
            if event.same_commit_files.files.is_empty()
                && event.same_commit_files.status.as_str() == "complete"
            {
                lines.push("      No other files changed.".to_owned());
            }
            for change in &event.same_commit_files.files {
                let old_path = change
                    .old_path
                    .as_deref()
                    .map(escape::path)
                    .unwrap_or_else(|| "—".to_owned());
                let new_path = change
                    .new_path
                    .as_deref()
                    .map(escape::path)
                    .unwrap_or_else(|| "—".to_owned());
                lines.push(format!(
                    "      {}  {} -> {}",
                    escape::subject(&change.status),
                    old_path,
                    new_path
                ));
            }
        }
    }
    lines.join("\n")
}

pub(super) fn format_json_report(
    report: &Report,
    warnings: &[String],
) -> Result<String, serde_json::Error> {
    let output = JsonReport {
        schema_version: 1,
        kind: "trace-removal",
        query: JsonQuery {
            code: &report.query,
            path: report.path.as_deref().map(json::json_path),
        },
        cache_tip: &report.cache_tip,
        scope: report.scope.as_ref().map(json::json_scope),
        limit: report.limit,
        matched_count: report.matched_count,
        truncated: report.truncated,
        events: report
            .events
            .iter()
            .map(|event| JsonEvent {
                commit_id: &event.commit_id,
                timestamp: &event.timestamp,
                message: json::json_path(&event.message),
                first_parent_id: &event.first_parent_id,
                old_path: json::json_path(&event.old_path),
                file_change: JsonFileChange {
                    status: &event.status,
                    old_path: json::json_path(&event.old_path),
                    new_path: event.new_path.as_deref().map(json::json_path),
                },
                same_commit_files: JsonSameCommitFiles {
                    status: event.same_commit_files.status.as_str(),
                    files: event
                        .same_commit_files
                        .files
                        .iter()
                        .map(|change| JsonOtherFileChange {
                            status: &change.status,
                            old_path: change.old_path.as_deref().map(json::json_path),
                            new_path: change.new_path.as_deref().map(json::json_path),
                        })
                        .collect(),
                },
                matches: event
                    .matches
                    .iter()
                    .map(|matched| JsonMatch {
                        line_number: matched.line_number,
                        line: json::json_path(&matched.line),
                    })
                    .collect(),
            })
            .collect(),
        warnings,
        notices: if report.events.is_empty() {
            Vec::new()
        } else {
            vec![SAME_COMMIT_NAVIGATION_NOTICE.to_owned()]
        },
    };
    serde_json::to_string(&output)
}

#[derive(Serialize)]
struct JsonReport<'a> {
    schema_version: u8,
    kind: &'static str,
    query: JsonQuery<'a>,
    cache_tip: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<JsonSearchScope<'a>>,
    limit: usize,
    matched_count: usize,
    truncated: bool,
    events: Vec<JsonEvent<'a>>,
    warnings: &'a [String],
    notices: Vec<String>,
}

#[derive(Serialize)]
struct JsonQuery<'a> {
    code: &'a str,
    path: Option<JsonPath<'a>>,
}

#[derive(Serialize)]
struct JsonEvent<'a> {
    commit_id: &'a str,
    timestamp: &'a str,
    message: JsonPath<'a>,
    first_parent_id: &'a str,
    old_path: JsonPath<'a>,
    file_change: JsonFileChange<'a>,
    same_commit_files: JsonSameCommitFiles<'a>,
    matches: Vec<JsonMatch<'a>>,
}

#[derive(Serialize)]
struct JsonFileChange<'a> {
    status: &'a str,
    old_path: JsonPath<'a>,
    new_path: Option<JsonPath<'a>>,
}

#[derive(Serialize)]
struct JsonSameCommitFiles<'a> {
    status: &'static str,
    files: Vec<JsonOtherFileChange<'a>>,
}

#[derive(Serialize)]
struct JsonOtherFileChange<'a> {
    status: &'a str,
    old_path: Option<JsonPath<'a>>,
    new_path: Option<JsonPath<'a>>,
}

#[derive(Serialize)]
struct JsonMatch<'a> {
    line_number: usize,
    line: JsonPath<'a>,
}
