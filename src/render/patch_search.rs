use crate::analysis::capabilities::patch_search::Report;

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
    let mut output = format!(
        "Patch equivalents for {} ({})\n{} matches; {} returned.\nScope: {} local/fetched branch tips; {} checked; {} indeterminate; {} unexamined; coverage complete: {}.\n",
        report.query.commit_id,
        report.query.integrity,
        report.matched_count,
        report.returned_count,
        report.scope.branch_tips.len(),
        report.scope.checked_count,
        report.scope.indeterminate.len(),
        report.scope.unexamined_count,
        report.scope.coverage_complete
    );
    if let Some(reason) = &report.query.reason {
        output.push_str(&format!("Query indeterminate: {reason}\n"));
    }
    if report.query.integrity == "empty" {
        output.push_str("No input patch: the selected commit has an empty patch.\n");
    }
    for result in &report.matches {
        output.push_str(&format!(
            "\n{} — {}\n  equivalent; basis: {} ({})\n",
            result.commit_id,
            super::escape::subject(&result.subject),
            result.comparison_basis.unwrap_or("unknown"),
            result.parent.as_deref().unwrap_or("root")
        ));
        for path in &result.paths {
            output.push_str(&format!(
                "  {}\n",
                super::escape::subject(&String::from_utf8_lossy(path))
            ));
        }
    }
    for gap in &report.scope.indeterminate {
        output.push_str(&format!(
            "\nIndeterminate {}: {}\n",
            gap.commit_id, gap.reason
        ));
    }
    output.push_str("\nComplete-patch content equality does not establish semantic equivalence or propagation direction.");
    output
}
