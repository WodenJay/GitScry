use std::collections::{HashMap, HashSet};

use crate::{
    app::AppError,
    cache::{HunkId, QuerySession},
    git::{WhyAnchor, WhyTarget},
};

use super::super::patch::{self, HunkPriorities};
use super::super::retrieval;
use super::super::{
    Report, ReportKind, SearchScopeInfo, WhyAttribution, WhyAttributionCommit, WhyModification,
    WhySummary,
};

const REMOTE_CONTEXT_NOTICE: &str = "Remote context unavailable from local history.";

pub(crate) fn run(
    session: &QuerySession,
    target: &WhyTarget,
    reachable: &HashSet<String>,
    eligible_revisions: Option<&HashSet<String>>,
    scope: Option<&SearchScopeInfo>,
    limit: usize,
    with_patch: bool,
) -> Result<Report, AppError> {
    let anchor_info = AnchorInfo::from_target(target);
    if !target.anchor_valid {
        let mut report = super::super::empty_report(ReportKind::Why);
        report.patch_mode = with_patch;
        report.notices.extend(target.warnings.iter().cloned());
        report.notices.push(REMOTE_CONTEXT_NOTICE.to_owned());
        let mut summary = empty_summary(target, &anchor_info, scope);
        summary.limitations = limitations(target, &anchor_info, &[], false, with_patch);
        report.why = Some(Box::new(summary));
        return Ok(report);
    }

    let commits = session.path_history(&target.path, reachable)?;

    let first_parent_history = session.first_parent_ancestors(&target.revision)?;
    let missing_objects = session.has_missing_objects(&commits)?;
    let mut start_line = anchor_info.line as i64;
    let mut end_line = anchor_info.symbol_end.unwrap_or(anchor_info.line) as i64;
    let mut target_modifications = HashMap::<String, (i64, WhyModification)>::new();
    let mut priorities = HunkPriorities::new();

    for commit in &commits {
        if !first_parent_history.contains(&commit.oid) {
            continue;
        }
        let hunks = session.history_hunks(&commit.oid)?;
        let range_start = start_line.min(end_line);
        let range_end = start_line.max(end_line);
        let related_hunks = if anchor_info.is_symbol() {
            let related = hunks
                .iter()
                .filter(|hunk| {
                    commit.anchored_ordinals.contains(&hunk.change_ordinal)
                        && retrieval::hunk_overlaps_symbol(hunk, range_start, range_end)
                })
                .map(|hunk| hunk.id())
                .collect::<Vec<_>>();
            retrieval::trace_line(commit, &hunks, &mut start_line);
            retrieval::trace_line(commit, &hunks, &mut end_line);
            related
        } else {
            retrieval::trace_line(commit, &hunks, &mut start_line)
                .into_iter()
                .collect::<Vec<_>>()
        };

        if with_patch {
            for hunk_id in &related_hunks {
                priorities
                    .entry(commit.oid.clone())
                    .or_default()
                    .insert(*hunk_id, 0);
            }
        }
        let eligible = eligible_revisions.is_none_or(|revisions| revisions.contains(&commit.oid));
        if related_hunks.is_empty() || !eligible || target_modifications.contains_key(&commit.oid) {
            continue;
        }

        let paths = paths_for_hunks(commit, &related_hunks);
        let mut basis = vec![anchor_info.modification_basis()];
        if commit.parent_count > 1 {
            basis.push("merge comparison uses the first parent".to_owned());
        }
        if commit.shallow_boundary {
            basis.push("history reaches a shallow boundary".to_owned());
        }
        target_modifications.insert(
            commit.oid.clone(),
            (
                commit.commit_time,
                WhyModification {
                    oid: commit.oid.clone(),
                    subject: commit.subject.clone(),
                    paths,
                    basis,
                    patch: None,
                },
            ),
        );
    }

    let attribution = attribution_for(
        target,
        &anchor_info,
        reachable,
        eligible_revisions,
        &target_modifications,
    );
    let consolidated_oid = match &attribution {
        WhyAttribution::Available(commit) if commit.consolidated_target_modification => {
            Some(commit.oid.as_str())
        }
        _ => None,
    };
    let attribution_oid = match &attribution {
        WhyAttribution::Available(commit) => Some(commit.oid.as_str()),
        _ => None,
    };
    let target_related_modification_count = target_modifications.len();
    let mut standalone = target_modifications
        .into_iter()
        .filter(|(oid, _)| Some(oid.as_str()) != consolidated_oid)
        .map(|(_, value)| value)
        .collect::<Vec<_>>();
    standalone.sort_by(|(left_time, left), (right_time, right)| {
        right_time
            .cmp(left_time)
            .then_with(|| left.oid.cmp(&right.oid))
    });
    let standalone_count = standalone.len();
    let target_related_modifications = standalone
        .into_iter()
        .take(limit)
        .map(|(_, modification)| modification)
        .collect::<Vec<_>>();
    let file_history_count = commits
        .iter()
        .filter(|commit| eligible_revisions.is_none_or(|revisions| revisions.contains(&commit.oid)))
        .count();
    let target_related_in_history = target_related_modification_count;
    let attribution_only_in_history = usize::from(
        attribution_oid.is_some_and(|oid| {
            commits.iter().any(|commit| {
                commit.oid == oid
                    && eligible_revisions.is_none_or(|revisions| revisions.contains(&commit.oid))
            })
        }) && matches!(
            &attribution,
            WhyAttribution::Available(commit) if !commit.consolidated_target_modification
        ),
    );
    let other_file_history_count =
        file_history_count.saturating_sub(target_related_in_history + attribution_only_in_history);
    let omitted_target_related_modifications =
        standalone_count.saturating_sub(target_related_modifications.len());

    let mut report = super::super::report(ReportKind::Why, Vec::new(), standalone_count, limit);
    report.notices.extend(target.warnings.iter().cloned());
    report.notices.push(REMOTE_CONTEXT_NOTICE.to_owned());
    if missing_objects {
        report.notices.push(
            "warning: local cache is missing Git objects; why material may be incomplete."
                .to_owned(),
        );
    }
    let mut summary = empty_summary(target, &anchor_info, scope);
    summary.target_related_modifications = target_related_modifications;
    summary.attribution = attribution;
    summary.standalone_target_related_modification_count = standalone_count;
    summary.other_file_history_count = other_file_history_count;
    summary.file_history_count = file_history_count;
    summary.omitted_target_related_modifications = omitted_target_related_modifications;
    summary.limitations = limitations(target, &anchor_info, &commits, missing_objects, with_patch);
    report.why = Some(Box::new(summary));
    if with_patch {
        patch::attach_why_patch_excerpts(session, &mut report, &priorities)?;
    }
    Ok(report)
}

fn attribution_for(
    target: &WhyTarget,
    anchor: &AnchorInfo<'_>,
    reachable: &HashSet<String>,
    eligible_revisions: Option<&HashSet<String>>,
    target_modifications: &HashMap<String, (i64, WhyModification)>,
) -> WhyAttribution {
    let Some(blame) = &target.blame else {
        return WhyAttribution::Unavailable {
            reason: unavailable_reason(target),
        };
    };
    if !reachable.contains(&blame.oid) {
        return WhyAttribution::Unavailable {
            reason: "Git blame's line owner is not reachable from the pinned target revision."
                .to_owned(),
        };
    }
    if eligible_revisions.is_some_and(|revisions| !revisions.contains(&blame.oid)) {
        return WhyAttribution::OutsideHistoricalScope;
    }

    let attribution_basis = anchor.attribution_basis();
    WhyAttribution::Available(WhyAttributionCommit {
        oid: blame.oid.clone(),
        subject: if blame.subject.is_empty() {
            "(subject unavailable)".to_owned()
        } else {
            blame.subject.clone()
        },
        shallow_boundary: blame.boundary,
        consolidated_target_modification: target_modifications.contains_key(&blame.oid),
        basis: vec![attribution_basis],
        patch: None,
    })
}

fn unavailable_reason(target: &WhyTarget) -> String {
    target.warnings.first().cloned().unwrap_or_else(|| {
        "Git line attribution is unavailable at this target revision.".to_owned()
    })
}

fn limitations(
    target: &WhyTarget,
    anchor: &AnchorInfo<'_>,
    commits: &[crate::cache::HistoryCommit],
    missing_objects: bool,
    with_patch: bool,
) -> Vec<String> {
    let mut limitations = vec![REMOTE_CONTEXT_NOTICE.to_owned()];
    if !target.anchor_valid {
        limitations.push(
            "The target anchor is unavailable at this revision; no modification classification was made."
                .to_owned(),
        );
        return limitations;
    }
    if anchor.is_symbol() {
        limitations.push(
            "Git blame covers only the symbol's starting line; target-related modification detection tracks the full symbol range."
                .to_owned(),
        );
    } else {
        limitations
            .push("Git blame identifies line ownership, not why that line was changed.".to_owned());
    }
    limitations.push(
        "Only changed diff lines overlapping the target anchor count as target-related; commit text and co-changed paths do not establish relevance."
            .to_owned(),
    );
    if commits.iter().any(|commit| commit.parent_count > 1) {
        limitations.push(
            "Target-line tracing follows first-parent history; changes reachable only from other merge parents are not classified."
                .to_owned(),
        );
    }
    if commits.iter().any(|commit| commit.shallow_boundary) {
        limitations
            .push("Shallow history may truncate attribution or modification tracing.".to_owned());
    }
    if commits.iter().any(|commit| {
        commit
            .changes
            .iter()
            .any(|change| change.status.starts_with('R') || change.status.starts_with('C'))
    }) {
        limitations.push(
            "File history follows detected renames; rename events alone are not target-related modifications."
                .to_owned(),
        );
    }
    if missing_objects {
        limitations.push(
            "Missing local Git objects may prevent diff corroboration; the absence of local diff corroboration does not establish that no relevant modification occurred."
                .to_owned(),
        );
    }
    if with_patch {
        limitations.push(
            "Patch excerpts use bounded cached text; an unavailable excerpt does not negate a classified target-related modification."
                .to_owned(),
        );
    }
    limitations
}

fn paths_for_hunks(commit: &crate::cache::HistoryCommit, related: &[HunkId]) -> Vec<Vec<u8>> {
    let mut paths = related
        .iter()
        .filter_map(|hunk| {
            commit
                .changes
                .iter()
                .find(|change| change.ordinal == hunk.change_ordinal)
                .and_then(|change| change.new_path.as_ref().or(change.old_path.as_ref()))
                .cloned()
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

struct AnchorInfo<'a> {
    label: String,
    kind: &'static str,
    line: usize,
    symbol_end: Option<usize>,
    attribution_scope: &'static str,
    symbol_name: Option<&'a str>,
}

impl<'a> AnchorInfo<'a> {
    fn from_target(target: &'a WhyTarget) -> Self {
        let (label, kind, line, attribution_scope, symbol_name) = match &target.anchor {
            WhyAnchor::Line { number } => (
                format!("line {number}"),
                "line",
                *number,
                "target_line",
                None,
            ),
            WhyAnchor::Symbol { name, number } => (
                format!("symbol {name} (line {number})"),
                "symbol",
                *number,
                "symbol_starting_line_only",
                Some(name.as_str()),
            ),
        };
        Self {
            label,
            kind,
            line,
            symbol_end: target.symbol_end,
            attribution_scope,
            symbol_name,
        }
    }

    fn is_symbol(&self) -> bool {
        self.symbol_name.is_some()
    }

    fn modification_basis(&self) -> String {
        match self.symbol_name {
            Some(name) => format!(
                "changed lines overlap symbol {name} (starting at line {}; full range tracked)",
                self.line
            ),
            None => format!("changed lines overlap target line {}", self.line),
        }
    }

    fn attribution_basis(&self) -> String {
        match self.symbol_name {
            Some(name) => format!(
                "Git blame assigns symbol {name}'s starting line ({}) to this commit",
                self.line
            ),
            None => format!("Git blame assigns target line {} to this commit", self.line),
        }
    }
}

fn empty_summary(
    target: &WhyTarget,
    anchor: &AnchorInfo<'_>,
    scope: Option<&SearchScopeInfo>,
) -> WhySummary {
    WhySummary {
        anchor: anchor.label.clone(),
        anchor_kind: anchor.kind,
        anchor_line: Some(anchor.line).filter(|line| *line > 0),
        symbol_end: anchor.symbol_end,
        attribution_scope: anchor.attribution_scope,
        revision: target.revision.clone(),
        target_related_modifications: Vec::new(),
        attribution: WhyAttribution::Unavailable {
            reason: unavailable_reason(target),
        },
        standalone_target_related_modification_count: 0,
        other_file_history_count: 0,
        file_history_count: 0,
        omitted_target_related_modifications: 0,
        timeline_follow_up_args: timeline_follow_up_args(&target.path, &target.revision, scope),
        limitations: Vec::new(),
    }
}

fn timeline_follow_up_args(
    path: &[u8],
    target_revision: &str,
    scope: Option<&SearchScopeInfo>,
) -> Option<Vec<String>> {
    let path = std::str::from_utf8(path).ok()?;
    let mut args = vec![
        "timeline".to_owned(),
        path.to_owned(),
        "--at".to_owned(),
        target_revision.to_owned(),
    ];
    if let Some(scope) = scope {
        if let Some(from_rev) = &scope.from_rev {
            args.extend(["--from-rev".to_owned(), from_rev.clone()]);
        }
        if scope.to_rev != target_revision {
            args.extend(["--to-rev".to_owned(), scope.to_rev.clone()]);
        }
        if let Some(since) = &scope.since {
            args.extend(["--since".to_owned(), since.clone()]);
        }
        if let Some(until) = &scope.until {
            args.extend(["--until".to_owned(), until.clone()]);
        }
    }
    Some(args)
}
