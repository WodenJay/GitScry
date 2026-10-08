use serde_json::json;

use super::{escape, json::json_path};
use crate::analysis::capabilities::followups::{Entry, Report};

pub(super) fn format_report(report: &Report) -> String {
    let scope = &report.scope;
    let mut lines = vec![
        format!(
            "Follow-up material for {} through {}",
            scope.seed, scope.endpoint
        ),
        format!(
            "Scope: cache tip {}; seed time {}; inclusive time ceiling {} ({} days); max-commits {}; limit {}; paths: {}.",
            scope.cache_tip,
            scope.seed_time,
            scope.time_ceiling,
            scope.days,
            scope.max_commits,
            scope.limit,
            scope
                .selected_paths
                .iter()
                .map(|p| escape::path(p))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        format!(
            "Inspected {} eligible commits (first {}, last {}), plus {} out-of-window lineage commits. {} matched in inspected scope; {} displayed. Traversal truncated: {}; display truncated: {}.",
            report.inspected_count,
            report.inspected_first.as_deref().unwrap_or("none"),
            report.inspected_last.as_deref().unwrap_or("none"),
            report.lineage_inspected_count,
            report.matched_in_inspected_scope,
            report.entries.len(),
            report.traversal_truncated,
            report.display_truncated
        ),
    ];
    let matching = &report.patch_matching;
    let seed_patch = &matching.seed_patch;
    lines.push(format!(
        "Patch matching: seed integrity {}; normalization v{}; comparison basis {}; paths {}; {} of {} eligible descendants checked; {} unexamined; coverage complete: {}. Complete-patch comparison is independent of --path.",
        seed_patch.integrity,
        seed_patch.normalization_version,
        seed_patch.comparison_basis.unwrap_or("unavailable"),
        if seed_patch.paths.is_empty() {
            "<none>".into()
        } else {
            seed_patch.paths.iter().map(|path| escape::path(path)).collect::<Vec<_>>().join(", ")
        },
        matching.checked_count,
        matching.eligible_count,
        matching.unexamined_count,
        matching.coverage_complete
    ));
    for gap in &matching.indeterminate {
        lines.push(format!(
            "Patch matching indeterminate for {}: {}.",
            gap.commit_id, gap.reason
        ));
    }
    if report.entries.is_empty() {
        lines.push(
            "No associated follow-up material in the inspected scope; this is not a stability conclusion."
                .into(),
        );
    }
    for entry in &report.entries {
        lines.push(format!(
            "{}  {}  {}",
            entry.commit_time,
            entry.commit_id,
            escape::subject(&entry.subject)
        ));
        let association_bases = association_bases(entry).collect::<Vec<_>>();
        let association_bases = if association_bases.is_empty() {
            "none".to_owned()
        } else {
            association_bases.join(", ")
        };
        lines.push(format!(
            "    association bases: {}; elapsed {} seconds; {}",
            association_bases,
            entry.elapsed_seconds,
            entry
                .file_associations
                .iter()
                .map(|association| {
                    let seed_old = association
                        .seed_old_path
                        .as_deref()
                        .map(escape::path)
                        .unwrap_or_else(|| "<created>".into());
                    let seed_new = association
                        .seed_new_path
                        .as_deref()
                        .map(escape::path)
                        .unwrap_or_else(|| "<deleted>".into());
                    let current = association
                        .current_path
                        .as_deref()
                        .map(escape::path)
                        .unwrap_or_else(|| "<deleted>".into());
                    format!(
                        "seed: {seed_old} -> {seed_new}; {} {} -> {current}",
                        association.change_type,
                        escape::path(&association.previous_path)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        ));
        for region in &entry.region_associations {
            let current_path = region
                .current_path
                .as_deref()
                .map(escape::path)
                .unwrap_or_else(|| "<deleted>".into());
            lines.push(format!(
                "    Region overlap: seed {}@{}:{}+{}; parent {}@{}:{}+{} -> {}@{}:{}+{}.",
                report.scope.seed,
                escape::path(&region.seed_path),
                region.seed_position.start_line,
                region.seed_position.line_count,
                region.previous_revision,
                escape::path(&region.previous_path),
                region.previous_position.start_line,
                region.previous_position.line_count,
                entry.commit_id,
                current_path,
                region.current_position.start_line,
                region.current_position.line_count
            ));
        }
        for downgrade in &entry.region_tracking_downgrades {
            let path = downgrade
                .current_path
                .as_deref()
                .unwrap_or(&downgrade.previous_path);
            lines.push(format!(
                "    Region tracking downgrade for {}: {} ({}).",
                escape::path(path),
                downgrade.reason.description(),
                downgrade.reason.code()
            ));
        }
        for relationship in &entry.patch_relationships {
            let seed_parent = relationship.seed_parent.as_deref().unwrap_or("<none>");
            let parent = relationship.parent.as_deref().unwrap_or("<none>");
            lines.push(format!(
                "    Patch relationship: {} (seed {}: {} parent {}; candidate {}: {} parent {}; normalization v{}; paths: {}).",
                relationship.relation,
                report.scope.seed,
                relationship.seed_comparison_basis,
                seed_parent,
                entry.commit_id,
                relationship.comparison_basis,
                parent,
                matching.seed_patch.normalization_version,
                relationship.paths.iter().map(|path| escape::path(path)).collect::<Vec<_>>().join(", ")
            ));
        }
        if let Some(reference) = &entry.revert_reference {
            lines.push(format!(
                "    Revert reference: commit-level declaration targeting {reference}; not path-specific and not proof of path reversal."
            ));
        }
        if entry.parent_count > 1 {
            lines.push("    Merge diff comparison: first parent.".into());
        }
        lines.push(format!("    Inspect with `git show {}`.", entry.commit_id));
        super::material::render_patch(&mut lines, entry.patch.as_ref(), "    ");
    }
    lines.join("\n")
}

fn association_bases(entry: &Entry) -> impl Iterator<Item = &'static str> {
    [
        entry
            .revert_reference
            .as_ref()
            .map(|_| "explicit_revert_reference"),
        (!entry.region_associations.is_empty()).then_some("region_overlap"),
        entry.same_file_association.then_some("same_file"),
        (!entry.patch_relationships.is_empty()).then_some("patch_relationship"),
    ]
    .into_iter()
    .flatten()
}

pub(super) fn format_json_report(
    report: &Report,
    warnings: &[String],
) -> Result<String, serde_json::Error> {
    let scope = &report.scope;
    let entries: Vec<_> = report
        .entries
        .iter()
        .map(|entry| {
            let association_bases = association_bases(entry).collect::<Vec<_>>();
            let basis = association_bases.first().copied();
            let mut material = json!({
                "commit_id": entry.commit_id,
                "subject": entry.subject,
                "commit_time": entry.commit_time,
                "elapsed_seconds": entry.elapsed_seconds,
                "basis": basis,
                "association_bases": association_bases,
                "patch_relationships": entry.patch_relationships.iter().map(|relationship| json!({
                    "relation": relationship.relation,
                    "seed_commit_id": &scope.seed,
                    "commit_id": &entry.commit_id,
                    "normalization_version": report.patch_matching.seed_patch.normalization_version,
                    "seed_comparison_basis": relationship.seed_comparison_basis,
                    "seed_parent": relationship.seed_parent,
                    "comparison_basis": relationship.comparison_basis,
                    "parent": relationship.parent,
                    "paths": relationship.paths.iter().map(|path| json_path(path)).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "paths": entry.paths.iter().map(|p| json_path(p)).collect::<Vec<_>>(),
                "change_types": entry.change_types,
                "file_associations": entry.file_associations.iter().map(|association| json!({
                    "seed_old_path": association.seed_old_path.as_ref().map(|path| json_path(path)),
                    "seed_new_path": association.seed_new_path.as_ref().map(|path| json_path(path)),
                    "previous_path": json_path(&association.previous_path),
                    "current_path": association.current_path.as_ref().map(|path| json_path(path)),
                    "change_type": association.change_type,
                })).collect::<Vec<_>>(),
                "region_associations": entry.region_associations.iter().map(|region| json!({
                    "seed_revision": &scope.seed,
                    "seed_path": json_path(&region.seed_path),
                    "commit_id": &entry.commit_id,
                    "previous_revision": region.previous_revision,
                    "previous_path": json_path(&region.previous_path),
                    "current_path": region.current_path.as_ref().map(|path| json_path(path)),
                    "seed_position": {"start_line": region.seed_position.start_line, "line_count": region.seed_position.line_count},
                    "previous_position": {"start_line": region.previous_position.start_line, "line_count": region.previous_position.line_count},
                    "current_position": {"start_line": region.current_position.start_line, "line_count": region.current_position.line_count},
                })).collect::<Vec<_>>(),
                "region_tracking_downgrades": entry.region_tracking_downgrades.iter().map(|downgrade| json!({
                    "seed_old_path": downgrade.seed_old_path.as_ref().map(|path| json_path(path)),
                    "seed_new_path": downgrade.seed_new_path.as_ref().map(|path| json_path(path)),
                    "previous_path": json_path(&downgrade.previous_path),
                    "current_path": downgrade.current_path.as_ref().map(|path| json_path(path)),
                    "reason": downgrade.reason.code(),
                    "explanation": downgrade.reason.description(),
                })).collect::<Vec<_>>(),
                "parent_count": entry.parent_count,
                "diff_comparison": if entry.parent_count > 1 { Some("first_parent") } else { None },
                "inspect_command": format!("git show {}", entry.commit_id),
            });
            if let Some(reference) = &entry.revert_reference {
                material["revert_reference"] = json!({
                    "target_commit_id": reference,
                    "scope": "commit_level",
                    "path_specific": false,
                });
            }
            if let Some(patch) = &entry.patch {
                material["patch"] = serde_json::to_value(super::json::json_patch(patch))?;
            }
            Ok(material)
        })
        .collect::<Result<_, serde_json::Error>>()?;
    serde_json::to_string(&json!({
        "schema_version": 4,
        "kind": "followups",
        "scope": {
            "seed": scope.seed, "endpoint": scope.endpoint, "cache_tip": scope.cache_tip,
            "seed_time": scope.seed_time, "time_ceiling": scope.time_ceiling,
            "days": scope.days, "max_commits": scope.max_commits, "limit": scope.limit,
            "selected_paths": scope.selected_paths.iter().map(|p| json_path(p)).collect::<Vec<_>>(),
            "order": "explicit_revert_reference_then_region_overlap_then_same_file_then_patch_relationship_then_forward_topological",
            "coverage": "endpoint_reachable_published_cache",
            "association": "explicit_revert_reference_or_region_overlap_or_same_file_or_patch_relationship",
        },
        "patch_matching": {
            "comparison": "complete_seed_and_candidate_commit_patches",
            "path_filter": "does_not_limit_patch_identity",
            "seed_patch": {
                "commit_id": scope.seed,
                "integrity": report.patch_matching.seed_patch.integrity,
                "reason": report.patch_matching.seed_patch.reason,
                "normalization_version": report.patch_matching.seed_patch.normalization_version,
                "comparison_basis": report.patch_matching.seed_patch.comparison_basis,
                "parent": report.patch_matching.seed_patch.parent,
                "paths": report.patch_matching.seed_patch.paths.iter().map(|path| json_path(path)).collect::<Vec<_>>(),
            },
            "eligible_count": report.patch_matching.eligible_count,
            "checked_count": report.patch_matching.checked_count,
            "unexamined_count": report.patch_matching.unexamined_count,
            "coverage_complete": report.patch_matching.coverage_complete,
            "indeterminate": report.patch_matching.indeterminate.iter().map(|item| json!({
                "commit_id": item.commit_id,
                "reason": item.reason,
            })).collect::<Vec<_>>(),
        },

        "inspected_count": report.inspected_count,
        "lineage_inspected_count": report.lineage_inspected_count,
        "inspected_first": report.inspected_first,
        "inspected_last": report.inspected_last,
        "matched_in_inspected_scope": report.matched_in_inspected_scope,
        "traversal_truncated": report.traversal_truncated,
        "display_truncated": report.display_truncated,
        "entries": entries,
        "warnings": warnings.iter().chain(&report.warnings).collect::<Vec<_>>(),
    }))
}
