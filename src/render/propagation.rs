use crate::analysis::capabilities::propagation::{Report, Status};

pub(super) fn format_report(report: &Report) -> String {
    let mut output = String::from("Propagation containment\n");
    output.push_str(&format!("source: {}\n", report.source));
    for target in &report.targets {
        output.push_str(&format!(
            "- {} ({}): {}",
            target.target_ref,
            target.target_oid,
            match target.status {
                Status::Contained => "contained",
                Status::Indeterminate => "indeterminate",
            }
        ));
        if let Some(reason) = &target.reason {
            output.push_str(&format!("; {reason}"));
        }
        output.push('\n');
        for commit in &target.contained_by {
            output.push_str(&format!("  contained by: {commit}\n"));
        }
    }
    output.push_str(&format!(
        "Coverage complete for requested targets: {}\n",
        report.coverage_complete
    ));
    output
}

pub(super) fn format_json_report(
    report: &Report,
    additional_warnings: &[String],
) -> Result<String, serde_json::Error> {
    let mut value = serde_json::to_value(report)?;
    let mut warnings = report.warnings.clone();
    warnings.extend_from_slice(additional_warnings);
    warnings.sort();
    warnings.dedup();
    value["warnings"] = serde_json::to_value(warnings)?;
    serde_json::to_string_pretty(&value)
}
