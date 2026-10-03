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
            format!(
                "reachable revisions {lower_revision} through {} inclusive; {committer_time}; published cache tip {}",
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

pub(super) fn format_report(report: &HotspotsReport) -> String {
    let mut lines = vec![
        scope_description(report),
        "Count distinct touching commits per target-present incarnation; creation, rename, binary and permission changes count. Revision/time bounds limit eligible touches; --path-prefix limits target-tree candidates; lineage follows full target history. No implicit path or large-commit exclusions.".into(),
        MERGE.into(),
        IDENTITY.into(),
        "Last changed: maximum eligible committer timestamp (UTC). Coverage: published cache and available local objects only; coverage warnings are reported separately.".into(),
        format!("Showing {} of {} files.", report.files.len(), report.total),
    ];
    for file in &report.files {
        lines.push(format!(
            "{}  {}  {}",
            file.touching_commits,
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
