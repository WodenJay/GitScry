use super::{escape, json, material};
use crate::analysis::capabilities::fragment_search::Report;

pub(super) fn format_report(report: &Report, links: Option<&crate::github::LinksReport>) -> String {
    let mut lines = vec![format!(
        "Showing {} of {} historical fragment occurrences (truncated: {}).",
        report.occurrences.len(),
        report.matched_count,
        report.truncated
    )];
    if let Some(scope) = &report.scope {
        lines.push(material::scope_summary(scope));
    }
    for occurrence in &report.occurrences {
        lines.push(format!(
            "{} {}:{}-{} {:?}",
            occurrence.commit_id,
            escape::path(&occurrence.path),
            occurrence.start_line,
            occurrence.end_line,
            occurrence.direction
        ));
        lines.push(format!(
            "Complete matched content: {}",
            escape::code_line(&occurrence.content)
        ));
        lines.push(format!(
            "Surrounding patch (truncated: {}; unavailable: {}):",
            occurrence.surrounding.truncated,
            occurrence.surrounding.text.is_none()
        ));
        if let Some(text) = &occurrence.surrounding.text {
            lines.push(escape::code_line(text));
        }
    }
    if let Some(links) = links {
        lines.push(super::github_links::format(links));
    }
    lines.join("\n")
}

pub(super) fn format_json_report(
    report: &Report,
    warnings: &[String],
    links: Option<&crate::github::LinksReport>,
) -> Result<String, serde_json::Error> {
    let occurrences: Vec<_> = report.occurrences.iter().map(|o| serde_json::json!({
        "commit_id": o.commit_id,
        "path": json::json_path(&o.path),
        "direction": match o.direction { crate::analysis::CodeDirection::Added => "added", crate::analysis::CodeDirection::Removed => "removed" },
        "start_line": o.start_line, "end_line": o.end_line,
        "content": json::json_path(&o.content),
        "surrounding": {
            "status": if o.surrounding.text.is_none() { "unavailable" } else if o.surrounding.truncated { "truncated" } else { "available" },
            "old_path": o.surrounding.old_path.as_deref().map(json::json_path),
            "new_path": o.surrounding.new_path.as_deref().map(json::json_path),
            "old_hunk_start": o.surrounding.old_hunk_start,
            "new_hunk_start": o.surrounding.new_hunk_start,
            "first_diff_row": o.surrounding.first_diff_row,
            "last_diff_row": o.surrounding.last_diff_row,
            "text": o.surrounding.text.as_deref().map(json::json_path),
            "truncated": o.surrounding.truncated,
            "unavailable": o.surrounding.text.is_none()
        }
    })).collect();
    serde_json::to_string(&serde_json::json!({
        "schema_version": 6, "kind": "code-fragment-search",
        "scope": report.scope.as_ref().map(json::json_scope),
        "matched_count": report.matched_count, "truncated": report.truncated,
        "occurrences": occurrences, "warnings": warnings,
        "github_links": links
    }))
}
