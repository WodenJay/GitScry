use super::json::json_path;
use crate::analysis::HotspotsReport;

const MERGE: &str = "Exclude merge contributions, including merge-only conflict resolution; include reachable non-merge branch commits and root introductions.";
const IDENTITY: &str = "Follow cached detected renames, not copies; deletion/recreation starts a new incarnation. Undetected renames and unavailable history cannot be recovered.";

fn scope_description(report: &HotspotsReport) -> String {
    let description = match &report.scope {
        Some(scope) => {
            let lower_revision = scope.from_rev.as_deref().map_or_else(
                || "history start".to_owned(),
                |revision| format!("after {revision}"),
            );
            let committer_time = match (scope.since.as_deref(), scope.until.as_deref()) {
                (Some(since), Some(until)) => {
                    format!("committer time {since} through {until} inclusive (UTC)")
                }
                (Some(since), None) => {
                    format!("committer time from {since} inclusive (UTC)")
                }
                (None, Some(until)) => {
                    format!("committer time through {until} inclusive (UTC)")
                }
                (None, None) => "unbounded committer time".to_owned(),
            };
            let coverage = if scope.coverage_complete {
                "complete"
            } else {
                "incomplete; only cached reachable history is included"
            };
            format!(
                "reachable revisions {lower_revision} through {} inclusive; {committer_time}; published cache tip {}; coverage {coverage}",
                scope.to_rev, scope.cache_tip
            )
        }
        None => "full reachable published history".to_owned(),
    };
    let path_prefix = report
        .path_prefix
        .as_ref()
        .map_or_else(String::new, |prefix| {
            format!("; target directory prefix `{prefix}`")
        });
    format!("Hotspots at {} ({description}{path_prefix})", report.target)
}
fn format_churn(amount: Option<u64>, complete: bool) -> String {
    match amount {
        Some(amount) if complete => amount.to_string(),
        Some(amount) => format!("{amount}*"),
        None => "—".into(),
    }
}

pub(super) fn format_report(report: &HotspotsReport) -> String {
    let mut lines = vec![
        scope_description(report),
        "Count distinct touching commits per target-present incarnation; creation, rename, binary and permission changes count. Revision/time bounds limit eligible touches; --path-prefix limits target-tree candidates; lineage follows full target history. No implicit path or large-commit exclusions.".into(),
        MERGE.into(),
        IDENTITY.into(),
        "Last changed: maximum eligible committer timestamp (UTC). Coverage: published cache and available local objects only; coverage warnings are reported separately.".into(),
        format!("Showing {} of {} files.", report.files.len(), report.total),
    ];
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
    let files: Vec<_> = report
        .files
        .iter()
        .map(|file| {
            serde_json::json!({
                "path": json_path(&file.path),
                "touching_commits": file.touching_commits,
                "last_changed": file.last_changed,
                "additions": file.additions,
                "deletions": file.deletions,
                "churn_complete": file.churn_complete,
            })
        })
        .collect();
    let scope = report.scope.as_ref();
    let scope_json = serde_json::json!({
        "target_rev": scope
            .and_then(|scope| scope.target_rev.as_deref())
            .unwrap_or(&report.target),
        "cache_tip": scope.map_or(&report.target, |scope| &scope.cache_tip),
        "to_rev": scope.map_or(&report.target, |scope| &scope.to_rev),
        "from_rev": scope.and_then(|scope| scope.from_rev.as_deref()),
        "since": scope.and_then(|scope| scope.since.as_deref()),
        "until": scope.and_then(|scope| scope.until.as_deref()),
        "coverage_complete": scope.map(|scope| scope.coverage_complete),
        "path_prefix": report.path_prefix.as_deref(),
        "history": if scope.is_some() {
            "eligible contributions use revision and committer-time bounds; lineage follows full target history"
        } else {
            "full reachable published history"
        },
    });
    serde_json::to_string(&serde_json::json!({
        "schema_version": 1,
        "kind": "hotspots",
        "scope": scope_json,
        "policy": {
            "counting": "distinct non-merge touching commits, including creation, pure rename, binary and permission changes",
            "merge": MERGE,
            "identity": IDENTITY,
            "last_changed": "maximum eligible committer timestamp in UTC",
            "textual_churn": "Sum cached textual diff lines for eligible touches; additions/deletions are null when none are calculable, and churn_complete is false when any eligible diff is unavailable",
            "exclusions": "no implicit path or large-commit exclusions",
            "ordering": "touching_commits descending, target path bytes ascending",
        },
        "coverage": {
            "basis": "published cache and available local objects only",
            "warnings": warnings,
            "rename_continuity": "bounded by cached detected renames",
        },
        "files": files,
        "total": report.total,
        "truncated": report.total > report.files.len(),
    }))
}
