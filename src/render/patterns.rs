use super::{escape, json::json_scope, material::scope_summary};
use crate::analysis::PatternsReport as Report;

const NOTICE: &str = "Historical co-change material, not a checklist of required edits. Detected Git rename records connect paths; deletion ends an incarnation, while recreation and copies start new ones. Undetected renames are not inferred.";

pub(super) fn format_report(report: &Report) -> String {
    let mut lines = vec![
        "Recurring file-incarnation combinations (all seeds)".to_owned(),
        format!("Target: {}", report.target_revision),
        format!(
            "Eligible seed commits: {}; excluded merges: {}; excluded >50-path commits: {}",
            report.eligible_seed_commits, report.excluded_merges, report.excluded_mass_changes
        ),
    ];
    if let Some(scope) = &report.scope {
        lines.push(scope_summary(scope));
    } else {
        lines.push("History: current HEAD-reachable commits in the published cache".to_owned());
    }
    lines.push(
        "Coverage: only eligible commits in the published cache are searched; uncached history is never searched."
            .to_owned(),
    );
    lines.push(NOTICE.to_owned());
    if report.patterns.is_empty() {
        lines.push("No recurring combinations meet the minimum support.".to_owned());
    }
    for (index, pattern) in report.patterns.iter().enumerate() {
        lines.push(format!(
            "\n{}. {} distinct commits; {:.1}% of eligible seed commits",
            index + 1,
            pattern.support_count,
            pattern.proportion * 100.0
        ));
        for member in &pattern.members {
            let target_paths = member
                .target_paths
                .iter()
                .map(|path| escape::path(path))
                .collect::<Vec<_>>();
            let presence = if target_paths.is_empty() {
                "missing at target".to_owned()
            } else {
                format!("present at target as {}", target_paths.join(", "))
            };
            lines.push(format!(
                "  {}{} [incarnation introduced at {}; {presence}]",
                escape::path(&member.path),
                if member.seed { " (seed)" } else { "" },
                member.introduced_in,
            ));
        }
        for citation in &pattern.citations {
            lines.push(format!("  commit: {}", citation.oid));
            for member in &citation.members {
                let changed_paths = member
                    .changed_paths
                    .iter()
                    .map(|path| escape::path(path))
                    .collect::<Vec<_>>()
                    .join(", ");
                lines.push(format!(
                    "    incarnation introduced at {} as {}; changed paths: {changed_paths}",
                    member.introduced_in,
                    escape::path(&member.introduced_path),
                ));
            }
        }
        if pattern.references_not_shown > 0 {
            lines.push(format!(
                "  {} supporting references not shown",
                pattern.references_not_shown
            ));
        }
    }
    if report.matched_count > report.patterns.len() {
        lines.push(format!(
            "{} combinations not shown",
            report.matched_count - report.patterns.len()
        ));
    }
    lines.join("\n")
}

pub(super) fn format_json_report(
    report: &Report,
    warnings: &[String],
) -> Result<String, serde_json::Error> {
    let patterns = report.patterns.iter().map(|pattern| serde_json::json!({
        "members": pattern.members.iter().map(|member| serde_json::json!({
            "path": escape::path(&member.path),
            "introduced_in": member.introduced_in,
            "seed": member.seed,
            "exists_at_target": member.exists_at_target,
            "target_paths": member.target_paths.iter().map(|path| escape::path(path)).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "support_count": pattern.support_count, "proportion": pattern.proportion,
        "citations": pattern.citations.iter().map(|citation| serde_json::json!({
            "oid": citation.oid,
            "commit_time": citation.commit_time,
            "members": citation.members.iter().map(|member| serde_json::json!({
                "introduced_in": member.introduced_in,
                "introduced_path": escape::path(&member.introduced_path),
                "changed_paths": member.changed_paths.iter().map(|path| escape::path(path)).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "references_not_shown": pattern.references_not_shown,
    })).collect::<Vec<_>>();
    serde_json::to_string_pretty(&serde_json::json!({
        "schema_version": 1, "kind": "patterns", "target_revision": report.target_revision,
        "scope": report.scope.as_ref().map(json_scope), "eligible_seed_commits": report.eligible_seed_commits,
        "history_coverage": "eligible commits intersected with published cache",
        "excluded_merges": report.excluded_merges, "excluded_mass_changes": report.excluded_mass_changes,
        "matched_count": report.matched_count, "returned_count": patterns.len(), "patterns": patterns,
        "warnings": warnings, "notices": [NOTICE],
    }))
}
