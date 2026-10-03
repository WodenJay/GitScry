use super::escape::subject as display;
use crate::analysis::capabilities::conflicts::{Region, Report};
use std::fmt::Write;

fn display_identities(identities: &[String]) -> String {
    identities
        .iter()
        .map(|identity| display(identity))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn format_report(report: &Report) -> String {
    let mut output = format!(
        "Merge conflict historical leads\nours: {}\ntheirs: {}\nmerge base: {}\nCoverage complete for selected supported paths: {}\n",
        report.ours, report.theirs, report.merge_base, report.coverage_complete
    );
    for file in &report.files {
        writeln!(output, "\nPath: {}", display(&file.path)).unwrap();
        writeln!(
            output,
            "  Conflict type: {}",
            if file.file_level {
                "File-level conflict"
            } else if file.unsupported.is_some() {
                "Unsupported conflict"
            } else {
                "Textual conflict"
            }
        )
        .unwrap();
        let index_stages = file
            .index_stages
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(output, "  Index stages: {index_stages}").unwrap();
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
            let available_paths = side
                .available_paths
                .iter()
                .map(|path| display(path))
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(
                output,
                "  Index stage {}: {}",
                side.index_stage,
                if side.index_stage_present {
                    "present"
                } else {
                    "absent"
                }
            )
            .unwrap();
            writeln!(
                output,
                "  Available paths: {}",
                if available_paths.is_empty() {
                    "none"
                } else {
                    &available_paths
                }
            )
            .unwrap();
            for lead in &side.leads {
                writeln!(output, "  {} {}", lead.commit, display(&lead.subject)).unwrap();
                writeln!(output, "    Side: {}", lead.side).unwrap();
                writeln!(
                    output,
                    "    Conflict path: {}",
                    display(&lead.conflict_path)
                )
                .unwrap();
                writeln!(output, "    Association: {}", lead.association).unwrap();
                writeln!(output, "    Selection basis: {}", lead.selection_basis).unwrap();
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
                for change in &lead.path_changes {
                    let old_path = change
                        .old_path
                        .as_deref()
                        .map(display)
                        .unwrap_or_else(|| "(absent)".to_owned());
                    let new_path = change
                        .new_path
                        .as_deref()
                        .map(display)
                        .unwrap_or_else(|| "(absent)".to_owned());
                    writeln!(
                        output,
                        "    Path change {} [{}]: {} -> {}",
                        change.change_ordinal, change.status, old_path, new_path
                    )
                    .unwrap();
                    let historical_path = change
                        .old_path
                        .as_deref()
                        .filter(|path| *path != lead.conflict_path.as_str())
                        .or_else(|| {
                            change
                                .new_path
                                .as_deref()
                                .filter(|path| *path != lead.conflict_path.as_str())
                        });
                    if let Some(path) = historical_path {
                        writeln!(output, "    Historical path: {}", display(path)).unwrap();
                    }
                }
                if lead.path_changes_truncated {
                    writeln!(output, "    Path changes truncated").unwrap();
                }
                for region in &lead.regions {
                    let old_path = region
                        .old_path
                        .as_deref()
                        .map(display)
                        .unwrap_or_else(|| "(absent)".to_owned());
                    let new_path = region
                        .new_path
                        .as_deref()
                        .map(display)
                        .unwrap_or_else(|| "(absent)".to_owned());
                    writeln!(
                        output,
                        "    {} change {} hunk {}: {} -> {}; old {}+{}, new {}+{}",
                        display(&lead.conflict_path),
                        region.change_ordinal,
                        region.hunk_ordinal,
                        old_path,
                        new_path,
                        region.old_start,
                        region.old_lines,
                        region.new_start,
                        region.new_lines
                    )
                    .unwrap();
                }
                if !lead.associated_material_ids.is_empty() {
                    let ids = lead
                        .associated_material_ids
                        .iter()
                        .map(|id| format!("#{id}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    writeln!(output, "    Associated material: {ids}").unwrap();
                }
                if lead.associated_materials_truncated {
                    writeln!(output, "    Associated material selection truncated").unwrap();
                }
                if lead.message_lossy {
                    writeln!(output, "    Message decoding: invalid UTF-8 bytes replaced").unwrap();
                }
                if lead.regions_truncated {
                    writeln!(output, "    Regions truncated").unwrap();
                }
            }
        }

        if file.related_history_total > 0 {
            writeln!(
                output,
                "  Shared history before merge base — {} of {} leads{}",
                file.related_history.len(),
                file.related_history_total,
                if file.related_history_truncated {
                    " (truncated)"
                } else {
                    ""
                }
            )
            .unwrap();
            for lead in &file.related_history {
                writeln!(output, "    {} {}", lead.commit, display(&lead.subject)).unwrap();
                writeln!(output, "      Association: {}", lead.association).unwrap();
                writeln!(
                    output,
                    "      Related sides: {}",
                    lead.related_sides.join(", ")
                )
                .unwrap();
                writeln!(
                    output,
                    "      Selection basis: {}",
                    lead.selection_basis.join("; ")
                )
                .unwrap();
                writeln!(
                    output,
                    "      Recorded reason [{}]: {}",
                    lead.reason_source,
                    lead.recorded_reason
                        .as_deref()
                        .map(display)
                        .unwrap_or_else(|| "none recorded; unknown".to_owned())
                )
                .unwrap();
                if let Some(revert) = &lead.reverted_by {
                    writeln!(
                        output,
                        "      Reverted by: {} {}",
                        revert.commit,
                        display(&revert.subject)
                    )
                    .unwrap();
                }
                if let Some(target) = &lead.reverts_commit {
                    writeln!(output, "      Reverts commit: {target}").unwrap();
                }
                write_regions(&mut output, "      ", &lead.path, &lead.regions);
                if lead.message_truncated {
                    writeln!(output, "      Message truncated").unwrap();
                }
                if lead.message_lossy {
                    writeln!(
                        output,
                        "      Message decoding: invalid UTF-8 bytes replaced"
                    )
                    .unwrap();
                }
                if lead.regions_truncated {
                    writeln!(output, "      Regions truncated").unwrap();
                }
            }
        }
    }
    if !report.associated_materials.is_empty() {
        writeln!(
            output,
            "\nAssociated material (same-commit historical associations; not dependency claims)"
        )
        .unwrap();
        for material in &report.associated_materials {
            writeln!(
                output,
                "  #{} {} [{}] in {}",
                material.id,
                display(&material.path),
                material.kind,
                material.commit
            )
            .unwrap();
            writeln!(output, "    Association: {}", material.association).unwrap();
            writeln!(
                output,
                "    Shared identities: {}",
                display_identities(&material.shared_identities)
            )
            .unwrap();
            for association in &material.associated_with {
                writeln!(
                    output,
                    "    Linked from {} {} lead {} via {}",
                    display(&association.conflict_path),
                    association.side,
                    association.lead_commit,
                    display_identities(&association.shared_identities)
                )
                .unwrap();
            }
            for hunk in &material.hunks {
                writeln!(
                    output,
                    "    Change {} hunk {} (old {}+{}, new {}+{}): {}",
                    hunk.change_ordinal,
                    hunk.hunk_ordinal,
                    hunk.old_start,
                    hunk.old_lines,
                    hunk.new_start,
                    hunk.new_lines,
                    display_identities(&hunk.shared_identities)
                )
                .unwrap();
                for line in &hunk.lines {
                    writeln!(output, "      {}", display(line)).unwrap();
                }
                if hunk.excerpt_truncated {
                    writeln!(output, "      [excerpt truncated]").unwrap();
                }
            }
            if material.hunks_truncated {
                writeln!(output, "    Associated hunks truncated").unwrap();
            }
            if material.associations_truncated {
                writeln!(output, "    Lead references truncated").unwrap();
            }
        }
    }
    if report.associated_materials_truncated {
        writeln!(output, "Associated material selection truncated").unwrap();
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

fn write_regions(output: &mut String, indentation: &str, path: &str, regions: &[Region]) {
    for region in regions {
        writeln!(
            output,
            "{indentation}{} change {} hunk {}: old {}+{}, new {}+{}",
            display(path),
            region.change_ordinal,
            region.hunk_ordinal,
            region.old_start,
            region.old_lines,
            region.new_start,
            region.new_lines
        )
        .unwrap();
    }
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
