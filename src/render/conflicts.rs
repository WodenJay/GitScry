use super::escape::subject as display;
use crate::analysis::capabilities::conflicts::Report;
use std::fmt::Write;

pub(super) fn format_report(report: &Report) -> String {
    let mut output = format!(
        "Merge conflict historical leads\nours: {}\ntheirs: {}\nmerge base: {}\nCoverage complete for selected supported paths: {}\n",
        report.ours, report.theirs, report.merge_base, report.coverage_complete
    );
    for file in &report.files {
        writeln!(output, "\nPath: {}", display(&file.path)).unwrap();
        if let Some(reason) = &file.unsupported {
            writeln!(output, "Unsupported: {}", display(reason)).unwrap();
        }
        for side in &file.sides {
            writeln!(
                output,
                "{} ({}) — {} of {} leads{}",
                side.name,
                side.endpoint,
                side.leads.len(),
                side.total_leads,
                if side.truncated { " (truncated)" } else { "" }
            )
            .unwrap();
            for lead in &side.leads {
                writeln!(output, "  {} {}", lead.commit, display(&lead.subject)).unwrap();
                writeln!(output, "    Association: {}", lead.association).unwrap();
                writeln!(
                    output,
                    "    Recorded reason [{}]: {}",
                    lead.reason_source,
                    lead.recorded_reason
                        .as_deref()
                        .map(display)
                        .unwrap_or_else(|| "none recorded in commit body".to_owned())
                )
                .unwrap();
                if lead.message_truncated {
                    writeln!(output, "    Message truncated").unwrap();
                }
                for region in &lead.regions {
                    writeln!(
                        output,
                        "    {} change {} hunk {}: old {}+{}, new {}+{}",
                        display(&lead.path),
                        region.change_ordinal,
                        region.hunk_ordinal,
                        region.old_start,
                        region.old_lines,
                        region.new_start,
                        region.new_lines
                    )
                    .unwrap();
                }
                if lead.regions_truncated {
                    writeln!(output, "    Regions truncated").unwrap();
                }
            }
        }
    }
    writeln!(
        output,
        "\nPaths: {} selected of {} unmerged{}",
        report.selected_paths,
        report.total_unmerged_paths,
        if report.files_truncated {
            " (file output truncated)"
        } else {
            ""
        }
    )
    .unwrap();
    for limitation in &report.limitations {
        writeln!(output, "Limitation: {limitation}").unwrap();
    }
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
