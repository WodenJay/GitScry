use super::{escape, json::json_scope, material::scope_summary};
use crate::analysis::PatternsReport as Report;

pub(super) fn format_report(report: &Report) -> String {
    let mut lines = vec![
        "Recurring concrete-path combinations (all seeds)".to_owned(),
        format!("Target: {}", report.target_revision),
        format!(
            "Eligible seed commits: {}; excluded merges: {}; excluded >50-path commits: {}",
            report.eligible_seed_commits, report.excluded_merges, report.excluded_mass_changes
        ),
    ];
    if let Some(scope) = &report.scope {
        lines.push(scope_summary(scope));
    }
    lines.push("Historical co-change material, not a checklist of required edits. Rename continuity is not inferred.".to_owned());
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
            lines.push(format!(
                "  {}{} [{} at target]",
                escape::path(&member.path),
                if member.seed { " (seed)" } else { "" },
                if member.exists_at_target {
                    "present"
                } else {
                    "missing"
                }
            ));
        }
        for (oid, _) in &pattern.citations {
            lines.push(format!("  commit: {oid}"));
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
        "members": pattern.members.iter().map(|member| serde_json::json!({"path": escape::path(&member.path), "seed": member.seed, "exists_at_target": member.exists_at_target})).collect::<Vec<_>>(),
        "support_count": pattern.support_count, "proportion": pattern.proportion,
        "citations": pattern.citations.iter().map(|(oid, time)| serde_json::json!({"oid": oid, "commit_time": time})).collect::<Vec<_>>(),
        "references_not_shown": pattern.references_not_shown,
    })).collect::<Vec<_>>();
    serde_json::to_string_pretty(&serde_json::json!({
        "schema_version": 1, "kind": "patterns", "target_revision": report.target_revision,
        "scope": report.scope.as_ref().map(json_scope), "eligible_seed_commits": report.eligible_seed_commits,
        "excluded_merges": report.excluded_merges, "excluded_mass_changes": report.excluded_mass_changes,
        "matched_count": report.matched_count, "returned_count": patterns.len(), "patterns": patterns,
        "warnings": warnings, "notices": ["Historical co-change material, not a checklist of required edits. Rename continuity is not inferred."],
    }))
}
