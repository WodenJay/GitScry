use super::json::json_path;
use crate::analysis::HotspotsReport;

const MERGE: &str = "Exclude merge contributions, including merge-only conflict resolution; include reachable non-merge branch commits and root introductions.";
const IDENTITY: &str = "Follow cached detected renames, not copies; deletion/recreation starts a new incarnation. Undetected renames and unavailable history cannot be recovered.";

fn format_churn(amount: Option<u64>, complete: bool) -> String {
    match amount {
        Some(amount) if complete => amount.to_string(),
        Some(amount) => format!("{amount}*"),
        None => "—".into(),
    }
}

pub(super) fn format_report(report: &HotspotsReport) -> String {
    let mut lines = vec![format!("Hotspots at {} (full published history)", report.target),
        "Count distinct touching commits per target-present incarnation; creation, rename, binary and permission changes count. No path or large-commit exclusions.".into(), MERGE.into(), IDENTITY.into(),
        "Last changed: maximum eligible committer timestamp (UTC). Coverage: published cache and available local objects only; coverage warnings are reported separately.".into(),
        format!("Showing {} of {} files.", report.files.len(), report.total)];

    lines.push("Touches  +lines  -lines  Last changed  Path".into());
    lines.push("Textual churn counts cached added/removed lines; * marks incomplete totals, — means none are calculable.".into());
    for file in &report.files {
        lines.push(format!(
            "{}  {}  {}  {}  {}",
            file.touching_commits,
            format_churn(file.additions, file.churn_complete),
            format_churn(file.deletions, file.churn_complete),
            file.last_changed,
            super::escape::path(&file.path)
        ));
    }
    if report.files.is_empty() {
        lines.push("No eligible file touches.".into());
    }
    lines.join("\n")
}
pub(super) fn format_json_report(
    report: &HotspotsReport,
    warnings: &[String],
) -> Result<String, serde_json::Error> {
    let files: Vec<_> = report.files.iter().map(|file| serde_json::json!({
        "path": json_path(&file.path), "touching_commits": file.touching_commits, "last_changed": file.last_changed, "additions": file.additions, "deletions": file.deletions, "churn_complete": file.churn_complete
    })).collect();
    serde_json::to_string(&serde_json::json!({
        "schema_version": 1, "kind": "hotspots",
        "scope": {"target_rev": report.target, "cache_tip": report.target, "to_rev": report.target, "from_rev": null, "since": null, "until": null, "history": "full reachable published history"},
        "policy": {"counting": "distinct non-merge touching commits, including creation, pure rename, binary and permission changes", "merge": MERGE, "identity": IDENTITY, "last_changed": "maximum eligible committer timestamp in UTC", "textual_churn": "Sum cached textual diff lines for eligible touches; additions/deletions are null when none are calculable, and churn_complete is false when any eligible diff is unavailable", "exclusions": "none", "ordering": "touching_commits descending, target path bytes ascending"},
        "coverage": {"basis": "published cache and available local objects only", "warnings": warnings, "rename_continuity": "bounded by cached detected renames"},
        "files": files, "total": report.total, "truncated": report.total > report.files.len()
    }))
}
