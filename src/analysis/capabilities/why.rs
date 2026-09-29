use std::collections::HashSet;

use crate::{
    app::AppError,
    cache::QuerySession,
    git::{WhyAnchor, WhyTarget},
};

use super::super::patch::HunkPriorities;
use super::super::retrieval;
use super::super::{Citation, Confidence, Detail, Material, Report, ReportKind, WhyDetail};

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

const REMOTE_CONTEXT_NOTICE: &str = "Remote context unavailable from local history.";

struct Entry {
    subject: String,
    paths: Vec<Vec<u8>>,
    confidence: Confidence,
    basis: Vec<String>,
    citations: Vec<Citation>,
}

pub(crate) fn run(
    session: &QuerySession,
    target: &WhyTarget,
    reachable: &HashSet<String>,
    eligible_revisions: Option<&HashSet<String>>,
    limit: usize,
) -> Result<Report, AppError> {
    if !target.anchor_valid {
        let mut report = super::super::empty_report(ReportKind::Why);
        report.notices.extend(target.warnings.iter().cloned());
        report.notices.push(REMOTE_CONTEXT_NOTICE.to_owned());
        return Ok(report);
    }
    let commits = session.path_history(&target.path, reachable)?;
    let missing_objects = session.has_missing_objects(&commits)?;
    let target_line = anchor_line(&target.anchor);
    let mut historical_line = target_line as i64;
    let blame_oid = target.blame.as_ref().map(|blame| blame.oid.as_str());
    let mut entries = Vec::new();
    let mut seen_blame = false;

    for commit in &commits {
        let hunks = session.history_hunks(&commit.oid)?;
        let direct_hunk = retrieval::trace_line(commit, &hunks, &mut historical_line).is_some();
        let blame_match = blame_oid == Some(commit.oid.as_str());
        let eligible = eligible_revisions.is_none_or(|revisions| revisions.contains(&commit.oid));
        seen_blame |= blame_match && eligible;
        if !eligible {
            continue;
        }
        let explanation = explanation_strength(&commit.subject, &commit.body);
        let cochanged = cochanged_paths(commit, &target.path);
        let mut basis = Vec::new();
        if explanation > 0 {
            basis.push("explanatory commit body".to_owned());
        }
        if blame_match {
            basis.push("target line blame identifies this commit".to_owned());
        }
        if direct_hunk {
            basis.push("diff hunk corroborates the target line".to_owned());
        }
        if cochanged > 0 {
            basis.push(format!("co-changed paths ({cochanged})"));
        }
        if commit.parent_count > 1 {
            basis.push("merge boundary: first-parent diff is the available comparison".to_owned());
        }
        if commit.changes.iter().any(|change| {
            commit.anchored_ordinals.contains(&change.ordinal)
                && (change.status.starts_with('R') || change.status.starts_with('C'))
        }) {
            basis.push("rename boundary: path history followed a move".to_owned());
        }
        if missing_objects && !direct_hunk {
            basis.push("missing local object prevented complete diff corroboration".to_owned());
        }
        let independent = blame_match || direct_hunk;
        let confidence = if explanation >= 2 && independent {
            Confidence::High
        } else if explanation > 0 && (independent || cochanged > 0) || independent && cochanged > 0
        {
            Confidence::Medium
        } else {
            Confidence::Low
        };
        let score = explanation as f64 * 8.0
            + if blame_match { 32.0 } else { 0.0 }
            + if direct_hunk { 16.0 } else { 0.0 }
            + (cochanged.min(5) as f64 * 1.5)
            + if commit.parent_count > 1 { 1.0 } else { 0.0 };
        let mut citations = vec![Citation::new(commit.oid.clone(), commit.subject.clone())];
        if let Some(blame) = &target.blame
            && blame.oid != commit.oid
            && eligible_revisions.is_none_or(|revisions| revisions.contains(&blame.oid))
        {
            citations.push(
                Citation::new(blame.oid.clone(), blame.subject.clone()).noting("target line blame"),
            );
        }
        entries.push(retrieval::Ranked {
            score,
            commit_time: commit.commit_time,
            oid: commit.oid.clone(),
            value: Entry {
                subject: commit.subject.clone(),
                paths: commit.paths.clone(),
                confidence,
                basis,
                citations,
            },
        });
    }

    if let Some(blame) = &target.blame
        && !seen_blame
        && eligible_revisions.is_none_or(|revisions| revisions.contains(&blame.oid))
    {
        let mut basis = vec!["target-specific blame fact outside the default cache".to_owned()];
        if blame.boundary {
            basis.push("shallow history boundary".to_owned());
        }
        if missing_objects {
            basis.push("missing local object prevented complete diff corroboration".to_owned());
        }
        entries.push(retrieval::Ranked {
            score: 7.0,
            commit_time: 0,
            oid: blame.oid.clone(),
            value: Entry {
                subject: if blame.subject.is_empty() {
                    "Target line owner (subject unavailable)".to_owned()
                } else {
                    blame.subject.clone()
                },
                paths: vec![target.path.clone()],
                confidence: Confidence::Low,
                basis,
                citations: vec![Citation::new(blame.oid.clone(), blame.subject.clone())],
            },
        });
    }

    let matched_count = entries.len();
    retrieval::sort(&mut entries);
    let mut materials = entries
        .into_iter()
        .take(limit)
        .map(|ranked| {
            let entry = ranked.value;
            Material {
                subject: entry.subject,
                paths: entry.paths,
                confidence: entry.confidence,
                basis: entry.basis,
                citations: entry.citations,
                detail: Some(Detail::Why(WhyDetail {
                    anchor: anchor_description(&target.anchor),
                    revision: target.revision.clone(),
                    line: target_line,
                })),
                patch: None,
            }
        })
        .collect::<Vec<_>>();
    retrieval::assign_citations(&mut materials);

    let mut report = super::super::report(ReportKind::Why, materials, matched_count, limit);
    report.notices.extend(target.warnings.iter().cloned());
    report.notices.push(REMOTE_CONTEXT_NOTICE.to_owned());
    if missing_objects {
        report.notices.push(
            "warning: local cache is missing Git objects; why material may be incomplete."
                .to_owned(),
        );
    }
    Ok(report)
}

pub(crate) fn patch_hunk_priorities(
    session: &QuerySession,
    target: &WhyTarget,
    reachable: &HashSet<String>,
) -> Result<HunkPriorities, AppError> {
    let mut priorities = HunkPriorities::new();
    if !target.anchor_valid {
        return Ok(priorities);
    }

    let commits = session.path_history(&target.path, reachable)?;
    match &target.anchor {
        WhyAnchor::Line { number } => {
            let mut line = *number as i64;
            for commit in commits {
                let hunks = session.history_hunks(&commit.oid)?;
                if let Some(id) = retrieval::trace_line(&commit, &hunks, &mut line) {
                    priorities.entry(commit.oid).or_default().insert(id, 0);
                    break;
                }
            }
        }
        WhyAnchor::Symbol { number, .. } => {
            let mut start = *number as i64;
            let mut end = target.symbol_end.unwrap_or(*number) as i64;
            for commit in commits {
                let hunks = session.history_hunks(&commit.oid)?;
                let anchored = commit
                    .anchored_ordinals
                    .iter()
                    .copied()
                    .collect::<HashSet<_>>();
                for hunk in &hunks {
                    if anchored.contains(&hunk.change_ordinal)
                        && retrieval::hunk_overlaps_symbol(hunk, start.min(end), start.max(end))
                    {
                        priorities
                            .entry(commit.oid.clone())
                            .or_default()
                            .insert(hunk.id(), 0);
                    }
                }
                let start_changed = retrieval::trace_line(&commit, &hunks, &mut start).is_some();
                let end_changed = retrieval::trace_line(&commit, &hunks, &mut end).is_some();
                // A signature-only edit changes one boundary without ending symbol history.
                if start_changed && end_changed {
                    break;
                }
            }
        }
    }
    Ok(priorities)
}

fn anchor_line(anchor: &WhyAnchor) -> usize {
    match anchor {
        WhyAnchor::Line { number } | WhyAnchor::Symbol { number, .. } => *number,
    }
}

fn anchor_description(anchor: &WhyAnchor) -> String {
    match anchor {
        WhyAnchor::Line { number } => format!("line {number}"),
        WhyAnchor::Symbol { name, number } => format!("symbol {name} (line {number})"),
    }
}

fn explanation_strength(_subject: &str, body: &str) -> usize {
    let body = body.trim();
    if body.is_empty() {
        return 0;
    }
    let lowered = body.to_ascii_lowercase();
    let markers = EXPLANATION_MARKERS
        .iter()
        .filter(|marker| lowered.contains(**marker))
        .count();
    let substantive = body.chars().count() >= 24;
    usize::from(substantive) + markers.min(2)
}

fn cochanged_paths(commit: &retrieval::HistoryCommit, target: &[u8]) -> usize {
    if commit.paths.len() <= 1 {
        return 0;
    }
    commit
        .paths
        .iter()
        .filter(|path| {
            if path.as_slice() == target {
                return false;
            }
            !commit.changes.iter().any(|change| {
                let rename = change.status.starts_with('R') || change.status.starts_with('C');
                rename
                    && ((change.old_path.as_deref() == Some(path.as_slice())
                        && change.new_path.as_deref() == Some(target))
                        || (change.new_path.as_deref() == Some(path.as_slice())
                            && change.old_path.as_deref() == Some(target)))
            })
        })
        .count()
}
