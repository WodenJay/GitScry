use crate::analysis::capabilities::patch_search::{Report, Scope};
use crate::analysis::capabilities::trace_fix::TraceFixVersions;

pub(super) fn format_json_report(
    report: &Report,
    warnings: &[String],
) -> Result<String, serde_json::Error> {
    let mut value = serde_json::to_value(report)?;
    value["warnings"] =
        serde_json::to_value(report.warnings.iter().chain(warnings).collect::<Vec<_>>())?;
    serde_json::to_string_pretty(&value)
}

pub(super) fn format_report(report: &Report) -> String {
    let mut output = format_patch_header(
        "Patch relationships",
        &query_label(&report.query),
        report.query.integrity,
        report.matched_count,
        report.returned_count,
        &report.scope,
    );
    if let Some(source) = &report.query.source {
        output.push_str(&format!(
            "Current input: {}; base HEAD: {}.\n",
            report.query.comparison_basis.unwrap_or("unknown"),
            source.head.as_deref().unwrap_or("none (unborn repository)")
        ));
    }
    append_query_state(
        &mut output,
        report.query.integrity,
        report.query.reason.as_deref(),
        report.query.source.is_some(),
    );
    for result in &report.matches {
        output.push_str(&format!(
            "\n{} — {}\n  {}; basis: {} ({})\n",
            result
                .commit_id
                .as_deref()
                .expect("historical patch result has a commit ID"),
            super::escape::subject(&result.subject),
            result.relation.unwrap_or("query"),
            result.comparison_basis.unwrap_or("unknown"),
            result.parent.as_deref().unwrap_or("root")
        ));
        for path in &result.paths {
            output.push_str(&format!("  {}\n", super::escape::path(path)));
        }
    }
    append_indeterminate(&mut output, &report.scope);
    append_disclaimer(&mut output);
    output
}

pub(super) fn format_trace_fix_versions(report: &TraceFixVersions) -> String {
    let mut output = format_patch_header(
        "Patch equivalents",
        &report.query.commit_id,
        report.query.integrity,
        report.matched_count,
        report.returned_count,
        &report.scope,
    );
    append_query_state(
        &mut output,
        report.query.integrity,
        report.query.reason.as_deref(),
        false,
    );
    for result in &report.matches {
        output.push_str(&format!(
            "\n{} — {}\n  equivalent; basis: {} ({})\n",
            result.commit_id,
            super::escape::subject(&result.subject),
            result.comparison_basis.unwrap_or("unknown"),
            result.parent.as_deref().unwrap_or("root")
        ));
        for path in &result.paths {
            output.push_str(&format!("  {}\n", super::escape::path(path)));
        }
        for hunk in &result.hunks {
            output.push_str(&format!(
                "  supporting patch hunk {} (operation {}; old {}+{}, new {}+{}):\n",
                hunk.ordinal,
                hunk.change_ordinal,
                hunk.old_start,
                hunk.old_lines,
                hunk.new_start,
                hunk.new_lines
            ));
            if let Some(text) = &hunk.text {
                for line in text
                    .as_bytes()
                    .split(|byte| *byte == b'\n')
                    .filter(|line| !line.is_empty())
                {
                    output.push_str(&format!("    {}\n", super::escape::code_line(line)));
                }
            }
            if hunk.text_truncated {
                output.push_str("    [supporting hunk text truncated]\n");
            }
        }
        if result.hunks_truncated {
            output.push_str("  [additional supporting hunks omitted]\n");
        }
    }
    append_indeterminate(&mut output, &report.scope);
    append_disclaimer(&mut output);
    output
}

fn query_label(query: &crate::analysis::capabilities::patch_search::PatchMaterial) -> String {
    if let Some(commit_id) = query.commit_id.as_deref() {
        return commit_id.to_owned();
    }
    match query.source.as_ref() {
        Some(source) if source.staged => "staged current change".to_owned(),
        Some(_) => "current working-tree change".to_owned(),
        None => "unknown patch input".to_owned(),
    }
}

fn format_patch_header(
    title: &str,
    query_commit_id: &str,
    query_integrity: &str,
    matched_count: usize,
    returned_count: usize,
    scope: &Scope,
) -> String {
    format!(
        "{} for {} ({})\n{} matches; {} returned.\nScope: {} local/fetched branch tips; {} checked; {} indeterminate; {} unexamined; coverage complete: {}.\n",
        title,
        query_commit_id,
        query_integrity,
        matched_count,
        returned_count,
        scope.branch_tips.len(),
        scope.checked_count,
        scope.indeterminate.len(),
        scope.unexamined_count,
        scope.coverage_complete
    )
}

fn append_query_state(
    output: &mut String,
    integrity: &str,
    reason: Option<&str>,
    current_input: bool,
) {
    if let Some(reason) = reason {
        output.push_str(&format!(
            "Query indeterminate: {}\n",
            super::escape::subject(reason)
        ));
    }
    if integrity == "empty" {
        let message = if current_input {
            "No input patch: the selected current change has no changes."
        } else {
            "No input patch: the selected commit has an empty patch."
        };
        output.push_str(message);
        output.push('\n');
    }
}

fn append_indeterminate(output: &mut String, scope: &Scope) {
    for gap in &scope.indeterminate {
        output.push_str(&format!(
            "\nIndeterminate {}: {}\n",
            gap.commit_id,
            super::escape::subject(&gap.reason)
        ));
    }
}

fn append_disclaimer(output: &mut String) {
    output.push_str(
        "\nComplete-patch content equality does not establish semantic equivalence or propagation direction.",
    );
}
