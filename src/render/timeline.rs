use serde::Serialize;

use super::{
    json::{self, JsonPatch, JsonPath, JsonSearchScope},
    material,
};
use crate::analysis::{TimelineEntry as Entry, TimelineReport as Report};
pub(super) fn format_report(report: &Report) -> String {
    let mut lines = vec![format!(
        "File evolution timeline for {} at {}",
        super::escape::path(&report.path),
        report.target_revision
    )];
    if let Some(scope) = &report.scope {
        lines.push(material::scope_summary(scope));
    }
    if report.total == 0 {
        lines.push(
            "No changes are recorded for this file incarnation in the published cache.".to_owned(),
        );
    } else if report.entries.is_empty() {
        lines.push(format!(
            "Empty page at offset {} ({} matching commits).",
            report.offset, report.total
        ));
    } else {
        lines.push(format!(
            "Showing {}-{} of {} matching commits (offset {}, limit {}).",
            report.start, report.end, report.total, report.offset, report.limit
        ));
        for entry in &report.entries {
            lines.push(format!(
                "{}  {}  {}  {}",
                entry.timestamp,
                entry.commit_id,
                entry.change_type,
                super::escape::path(&entry.path)
            ));
            lines.push(format!("    {}", super::escape::subject(&entry.subject)));
            if entry.shallow_boundary {
                lines.push(
                    "    History reaches a shallow boundary; this may not be the file's introduction."
                        .to_owned(),
                );
            }
            if entry.parent_count > 1 {
                lines.push("    Merge diff is compared with the first parent.".to_owned());
            }
            lines.push(format!(
                "    Inspect commit/diff with `git show {}`.",
                entry.commit_id
            ));
            super::material::render_patch(&mut lines, entry.patch.as_ref(), "    ");
        }
    }
    lines.join("\n")
}

pub(super) fn format_json_report(
    report: &Report,
    additional_warnings: &[String],
    github_links: Option<&crate::github::LinksReport>,
) -> Result<String, serde_json::Error> {
    let entries = report
        .entries
        .iter()
        .map(JsonEntry::from)
        .collect::<Vec<_>>();
    let output = JsonReport {
        schema_version: if github_links.is_some() {
            3
        } else if report.patch_mode {
            2
        } else {
            1
        },
        kind: "timeline",
        target_revision: &report.target_revision,
        path: json::json_path(&report.path),
        total: report.total,
        offset: report.offset,
        limit: report.limit,
        start: report.start,
        end: report.end,
        has_more: report.has_more,
        entries,
        warnings: additional_warnings.iter().collect(),
        notices: Vec::new(),
        github_links,
        scope: report.scope.as_ref().map(json::json_scope),
    };
    serde_json::to_string(&output)
}

#[derive(Serialize)]
struct JsonReport<'a> {
    schema_version: u8,
    kind: &'static str,
    target_revision: &'a str,
    path: JsonPath<'a>,
    total: usize,
    offset: usize,
    limit: usize,
    start: usize,
    end: usize,
    has_more: bool,
    entries: Vec<JsonEntry<'a>>,
    warnings: Vec<&'a String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<JsonSearchScope<'a>>,
    notices: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    github_links: Option<&'a crate::github::LinksReport>,
}

#[derive(Serialize)]
struct JsonEntry<'a> {
    commit_id: &'a str,
    timestamp: &'a str,
    subject: &'a str,
    path: JsonPath<'a>,
    change_type: &'static str,
    parent_count: usize,
    shallow_boundary: bool,
    diff_comparison: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    patch: Option<JsonPatch<'a>>,
}

impl<'a> From<&'a Entry> for JsonEntry<'a> {
    fn from(entry: &'a Entry) -> Self {
        Self {
            commit_id: &entry.commit_id,
            timestamp: &entry.timestamp,
            subject: &entry.subject,
            path: json::json_path(&entry.path),
            change_type: entry.change_type,
            parent_count: entry.parent_count,
            shallow_boundary: entry.shallow_boundary,
            diff_comparison: (entry.parent_count > 1).then_some("first_parent"),
            patch: entry.patch.as_ref().map(json::json_patch),
        }
    }
}
