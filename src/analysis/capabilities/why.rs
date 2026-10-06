use std::collections::{HashMap, HashSet};

use crate::{
    app::AppError,
    cache::{HunkId, QuerySession},
    git::{SymbolTrace, WhyAnchor, WhyTarget},
};

use super::super::patch::{self, HunkPriorities};
use super::super::retrieval;
use super::super::{PatchExcerpt, Report, ReportKind, SearchScopeInfo};
use crate::analysis::query::{Context, Options, Outcome, QueryReport, scope};
use crate::git::Repository;

pub(in crate::analysis) fn execute(
    revision: Option<String>,
    path: String,
    anchor: WhyAnchor,
    options: Options,
) -> Result<Outcome, AppError> {
    let repository = Repository::discover()?;
    let implicit_target = revision.is_none();
    let (context, target) = Context::prepare_target(
        &repository,
        revision.as_deref(),
        options.scope,
        |revision| repository.pin_why_target(revision, &path, anchor),
    )?;
    let reachable = if implicit_target {
        scope::reachable_history(&context.session, &repository, &target.revision)?
            .revisions
            .into_iter()
            .collect()
    } else {
        context.session.ancestors(&target.revision)?
    };
    let eligible = context.eligible_revisions(&target.revision)?;
    let report = run(
        &context.session,
        &target,
        &reachable,
        eligible.as_ref(),
        context.scope.as_ref().map(|scope| &scope.report),
        options.limit,
        options.patch,
    )?;
    Ok(context.finish(QueryReport::Analysis(report)))
}

/// The separate facts returned by a line or symbol `why` query.
pub(crate) struct WhySummary {
    pub(crate) anchor: String,
    pub(crate) anchor_kind: &'static str,
    pub(crate) anchor_line: Option<usize>,
    pub(crate) symbol_end: Option<usize>,
    pub(crate) attribution_scope: &'static str,
    pub(crate) revision: String,
    pub(crate) target_related_modifications: Vec<WhyModification>,
    pub(crate) attribution: WhyAttribution,
    pub(crate) standalone_target_related_modification_count: usize,
    pub(crate) other_file_history_count: usize,
    pub(crate) file_history_count: usize,
    pub(crate) omitted_target_related_modifications: usize,
    pub(crate) timeline_follow_up_args: Option<Vec<String>>,
    pub(crate) limitations: Vec<String>,
}

pub(crate) struct WhyModification {
    pub(crate) oid: String,
    pub(crate) subject: String,
    pub(crate) paths: Vec<Vec<u8>>,
    pub(crate) basis: Vec<String>,
    pub(crate) patch: Option<PatchExcerpt>,
}

pub(crate) enum WhyAttribution {
    Available(WhyAttributionCommit),
    OutsideHistoricalScope,
    Unavailable { reason: String },
}

pub(crate) struct WhyAttributionCommit {
    pub(crate) oid: String,
    pub(crate) subject: String,
    pub(crate) shallow_boundary: bool,
    pub(crate) consolidated_target_modification: bool,
    pub(crate) basis: Vec<String>,
    pub(crate) patch: Option<PatchExcerpt>,
}

pub(crate) struct SymbolSummary {
    pub(crate) target: String,
    pub(crate) introduction: SymbolFact,
    pub(crate) anchor_line_attribution: SymbolFact,
}

pub(crate) enum SymbolFact {
    Known { commit_oid: String, subject: String },
    Unknown { reason: String },
}

const REMOTE_CONTEXT_NOTICE: &str = "Remote context unavailable from local history.";

const EXPLANATION_MARKERS: &[&str] = &[
    "because",
    "so that",
    "to avoid",
    "prevents",
    "otherwise",
    "regression",
    "incident",
    "stale",
    "issue #",
    "fixes",
    "caused",
];
fn run(
    session: &QuerySession,
    target: &WhyTarget,
    reachable: &HashSet<String>,
    eligible_revisions: Option<&HashSet<String>>,
    scope: Option<&SearchScopeInfo>,
    limit: usize,
    with_patch: bool,
) -> Result<Report, AppError> {
    let anchor_info = AnchorInfo::from_target(target);
    let unavailable_trace = SymbolTrace {
        revisions: Vec::new(),
        modifications: Vec::new(),
        paths: Vec::new(),
        introduction: Err("Git symbol-range history is unavailable".to_owned()),
        warnings: Vec::new(),
    };
    let symbol_trace = anchor_info
        .is_symbol()
        .then(|| target.symbol_trace.as_ref().unwrap_or(&unavailable_trace));
    if !target.anchor_valid {
        let mut report = super::super::empty_report(ReportKind::Why);
        report.patch_mode = with_patch;
        report.notices.extend(target.warnings.iter().cloned());
        report.notices.push(REMOTE_CONTEXT_NOTICE.to_owned());
        let mut summary = empty_summary(target, &anchor_info, scope);
        summary.limitations = limitations(target, &anchor_info, &[], false, with_patch);
        report.why = Some(Box::new(summary));
        if let Some(trace) = symbol_trace {
            let commits = session.path_history(&target.path, reachable)?;
            let missing_objects = session.has_missing_objects(&commits)?;
            report.symbol_summary = Some(summarize_symbol(
                target,
                &anchor_info,
                &commits,
                reachable,
                eligible_revisions,
                missing_objects,
                trace,
            ));
        }
        return Ok(report);
    }

    let mut commits = session.path_history(&target.path, reachable)?;
    if let Some(trace) = symbol_trace {
        let mut seen = commits
            .iter()
            .map(|commit| commit.oid.clone())
            .collect::<HashSet<_>>();
        let paths = trace.paths.iter().collect::<HashSet<_>>();
        for path in paths {
            for commit in session.path_history(path, reachable)? {
                if seen.insert(commit.oid.clone()) {
                    commits.push(commit);
                }
            }
        }
    }

    let first_parent_history = session.first_parent_ancestors(&target.revision)?;
    let missing_objects = session.has_missing_objects(&commits)?;
    let trace_revisions = symbol_trace.as_ref().map(|trace| {
        trace
            .modifications
            .iter()
            .map(|change| change.oid.clone())
            .collect::<HashSet<_>>()
    });
    let symbol_summary = symbol_trace.map(|trace| {
        summarize_symbol(
            target,
            &anchor_info,
            &commits,
            reachable,
            eligible_revisions,
            missing_objects,
            trace,
        )
    });
    let confirmed_introduction =
        symbol_summary
            .as_ref()
            .and_then(|summary| match &summary.introduction {
                SymbolFact::Known { commit_oid, .. } => Some(commit_oid.as_str()),
                SymbolFact::Unknown { .. } => None,
            });
    let mut start_line = anchor_info.line as i64;
    let mut target_modifications = HashMap::<String, (i64, WhyModification)>::new();
    let mut symbol_scores = HashMap::<String, usize>::new();
    let mut priorities = HunkPriorities::new();

    for commit in &commits {
        if !first_parent_history.contains(&commit.oid) {
            continue;
        }
        let hunks = session.history_hunks(&commit.oid)?;
        let related_hunks = if anchor_info.is_symbol() {
            let change = symbol_trace.and_then(|trace| {
                trace
                    .modifications
                    .iter()
                    .find(|change| change.oid == commit.oid)
            });
            hunks
                .iter()
                .filter(|hunk| {
                    change.is_some_and(|change| {
                        commit.changes.iter().any(|path| {
                            path.ordinal == hunk.change_ordinal
                                && path.new_path.as_ref().or(path.old_path.as_ref())
                                    == Some(&change.path)
                        }) && retrieval::hunk_overlaps_symbol(
                            hunk,
                            change.start as i64,
                            change.end as i64,
                        )
                    })
                })
                .map(|hunk| hunk.id())
                .collect::<Vec<_>>()
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
        let is_symbol_change = trace_revisions
            .as_ref()
            .is_some_and(|revisions| revisions.contains(&commit.oid));
        let introduction = confirmed_introduction == Some(commit.oid.as_str());
        let relevant_change = if anchor_info.is_symbol() {
            is_symbol_change && !introduction
        } else {
            !related_hunks.is_empty()
        };
        if !relevant_change || !eligible || target_modifications.contains_key(&commit.oid) {
            continue;
        }

        let paths = if related_hunks.is_empty() {
            vec![target.path.clone()]
        } else {
            paths_for_hunks(commit, &related_hunks)
        };
        let mut basis = vec![anchor_info.modification_basis()];
        let explanation = if anchor_info.is_symbol() {
            explanation_strength(&commit.body)
        } else {
            0
        };
        if explanation > 0 {
            basis.push("explanatory commit body".to_owned());
        }
        if anchor_info.is_symbol() {
            basis.push("Git range tracing identifies a change to this symbol".to_owned());
        }
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
        if anchor_info.is_symbol() {
            symbol_scores.insert(commit.oid.clone(), explanation);
        }
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
    let attribution_is_modification =
        attribution_oid.is_some_and(|oid| target_modifications.contains_key(oid));
    let target_related_modification_count = target_modifications.len();
    let mut standalone = target_modifications
        .into_iter()
        .filter(|(oid, _)| Some(oid.as_str()) != consolidated_oid)
        .map(|(_, value)| value)
        .collect::<Vec<_>>();
    if anchor_info.is_symbol() {
        standalone.sort_by(|(left_time, left), (right_time, right)| {
            let left_score = symbol_scores.get(&left.oid).copied().unwrap_or_default();
            let right_score = symbol_scores.get(&right.oid).copied().unwrap_or_default();
            right_score
                .cmp(&left_score)
                .then_with(|| right_time.cmp(left_time))
                .then_with(|| left.oid.cmp(&right.oid))
        });
    } else {
        standalone.sort_by(|(left_time, left), (right_time, right)| {
            right_time
                .cmp(left_time)
                .then_with(|| left.oid.cmp(&right.oid))
        });
    }
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
        }) && !attribution_is_modification
            && matches!(
                &attribution,
                WhyAttribution::Available(commit) if !commit.consolidated_target_modification
            ),
    );
    let other_file_history_count =
        file_history_count.saturating_sub(target_related_in_history + attribution_only_in_history);
    let omitted_target_related_modifications =
        standalone_count.saturating_sub(target_related_modifications.len());

    let mut report = super::super::report(ReportKind::Why, Vec::new(), standalone_count, limit);
    report.symbol_selection = target.symbol_selection.clone().map(Box::new);
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
    report.symbol_summary = symbol_summary;
    if with_patch {
        patch::attach_why_patch_excerpts(session, &mut report, &priorities)?;
    }
    Ok(report)
}

fn summarize_symbol(
    target: &WhyTarget,
    anchor: &AnchorInfo<'_>,
    commits: &[crate::cache::HistoryCommit],
    reachable: &HashSet<String>,
    eligible_revisions: Option<&HashSet<String>>,
    missing_objects: bool,
    trace: &SymbolTrace,
) -> SymbolSummary {
    use retrieval::target_history::SymbolHistoryLimitation;

    let introduction = match retrieval::target_history::confirm_symbol_introduction(
        trace,
        commits,
        reachable,
        target.shallow,
        missing_objects,
        trace.revisions.iter(),
    ) {
        Err(SymbolHistoryLimitation::Trace(reason)) => SymbolFact::Unknown {
            reason: reason.to_owned(),
        },
        Err(SymbolHistoryLimitation::CacheGap) => SymbolFact::Unknown {
            reason: "symbol history extends beyond published cache coverage".to_owned(),
        },
        Err(SymbolHistoryLimitation::MissingObjects) => SymbolFact::Unknown {
            reason: "local Git objects needed to confirm the introduction are missing".to_owned(),
        },
        Err(SymbolHistoryLimitation::UncertainLineage) => SymbolFact::Unknown {
            reason: "shallow, merge, or path-move history makes symbol lineage uncertain"
                .to_owned(),
        },
        Err(SymbolHistoryLimitation::IntroductionAbsent) => SymbolFact::Unknown {
            reason: "symbol introduction is not present in cached path history".to_owned(),
        },
        Ok(commit) if !eligible_revisions.is_none_or(|eligible| eligible.contains(&commit.oid)) => {
            SymbolFact::Unknown {
                reason: "symbol introduction is outside the current query scope".to_owned(),
            }
        }
        Ok(commit) => SymbolFact::Known {
            commit_oid: commit.oid.clone(),
            subject: commit.subject.clone(),
        },
    };
    let anchor_line_attribution = match &target.blame {
        None => SymbolFact::Unknown {
            reason: "Git could not attribute the symbol anchor line".to_owned(),
        },
        Some(blame) if !reachable.contains(&blame.oid) => SymbolFact::Unknown {
            reason: "Git blame's line owner is not reachable from the pinned target revision."
                .to_owned(),
        },
        Some(blame) if !eligible_revisions.is_none_or(|eligible| eligible.contains(&blame.oid)) => {
            SymbolFact::Unknown {
                reason: "anchor-line attribution is outside the current query scope".to_owned(),
            }
        }
        Some(blame) => SymbolFact::Known {
            commit_oid: blame.oid.clone(),
            subject: blame.subject.clone(),
        },
    };
    SymbolSummary {
        target: anchor.label.clone(),
        introduction,
        anchor_line_attribution,
    }
}

fn explanation_strength(body: &str) -> usize {
    let body = body.trim();
    if body.is_empty() {
        return 0;
    }
    let lowered = body.to_ascii_lowercase();
    let markers = EXPLANATION_MARKERS
        .iter()
        .filter(|marker| lowered.contains(**marker))
        .count();
    usize::from(body.chars().count() >= 24) + markers.min(2)
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
        consolidated_target_modification: !anchor.is_symbol()
            && target_modifications.contains_key(&blame.oid),
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
    limitations.extend(target.warnings.iter().cloned());
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
