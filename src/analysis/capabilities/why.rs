use std::collections::{HashMap, HashSet};

use crate::{
    app::AppError,
    cache::QuerySession,
    git::{SymbolTrace, WhyAnchor, WhyTarget},
};

use super::super::patch::{self, HunkPriorities};
use super::super::retrieval;
use super::super::{
    Citation, Confidence, Detail, Material, Report, ReportKind, SymbolFact, SymbolSummary,
    WhyDetail,
};

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
    with_patch: bool,
    symbol_trace: Option<SymbolTrace>,
) -> Result<Report, AppError> {
    if !target.anchor_valid {
        let mut report = super::super::empty_report(ReportKind::Why);
        report.patch_mode = with_patch;
        report.notices.extend(target.warnings.iter().cloned());
        report.notices.push(REMOTE_CONTEXT_NOTICE.to_owned());
        return Ok(report);
    }
    if matches!(&target.anchor, WhyAnchor::Symbol { .. }) {
        return run_symbol(
            session,
            target,
            reachable,
            eligible_revisions,
            limit,
            with_patch,
            symbol_trace.unwrap_or_else(|| SymbolTrace {
                revisions: Vec::new(),
                introduction: Err("Git symbol-range history is unavailable".to_owned()),
            }),
        );
    }
    let commits = session.path_history(&target.path, reachable)?;
    let missing_objects = session.has_missing_objects(&commits)?;
    let target_line = anchor_line(&target.anchor);
    let mut historical_line = target_line as i64;
    let blame_oid = target.blame.as_ref().map(|blame| blame.oid.as_str());
    let mut entries = Vec::new();
    let mut seen_blame = false;
    let mut priorities = HunkPriorities::new();
    // Excerpts stop at line ownership (or symbol introduction); ranking keeps walking.
    let mut select_patch = with_patch;
    let mut symbol_end = target.symbol_end.unwrap_or(target_line) as i64;

    for commit in &commits {
        let hunks = session.history_hunks(&commit.oid)?;
        if select_patch && matches!(target.anchor, WhyAnchor::Symbol { .. }) {
            for hunk in &hunks {
                if commit.anchored_ordinals.contains(&hunk.change_ordinal)
                    && retrieval::hunk_overlaps_symbol(
                        hunk,
                        historical_line.min(symbol_end),
                        historical_line.max(symbol_end),
                    )
                {
                    priorities
                        .entry(commit.oid.clone())
                        .or_default()
                        .insert(hunk.id(), 0);
                }
            }
        }
        let direct_hunk = retrieval::trace_line(commit, &hunks, &mut historical_line);
        if select_patch {
            match target.anchor {
                WhyAnchor::Line { .. } => {
                    if let Some(id) = direct_hunk {
                        priorities
                            .entry(commit.oid.clone())
                            .or_default()
                            .insert(id, 0);
                        select_patch = false;
                    }
                }
                WhyAnchor::Symbol { .. } => {
                    let end_changed =
                        retrieval::trace_line(commit, &hunks, &mut symbol_end).is_some();
                    // A signature-only edit changes one endpoint, not the whole symbol.
                    select_patch = !(direct_hunk.is_some() && end_changed);
                }
            }
        }
        let direct_hunk = direct_hunk.is_some();
        // Scope controls output eligibility, never the positions or ownership we traced.
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
    if with_patch {
        patch::attach_selected_patch_excerpts(session, &mut report, &priorities)?;
    }
    Ok(report)
}

fn run_symbol(
    session: &QuerySession,
    target: &WhyTarget,
    reachable: &HashSet<String>,
    eligible_revisions: Option<&HashSet<String>>,
    limit: usize,
    with_patch: bool,
    trace: SymbolTrace,
) -> Result<Report, AppError> {
    let commits = session.path_history(&target.path, reachable)?;
    let missing_objects = session.has_missing_objects(&commits)?;
    let commits_by_oid = commits
        .iter()
        .map(|commit| (commit.oid.as_str(), commit))
        .collect::<HashMap<_, _>>();
    let SymbolTrace {
        revisions,
        introduction,
    } = trace;
    let trace_revisions = revisions.iter().map(String::as_str).collect::<HashSet<_>>();
    let cache_gap = revisions
        .iter()
        .any(|oid| !reachable.contains(oid) || !commits_by_oid.contains_key(oid.as_str()));
    let uncertain_lineage = revisions
        .iter()
        .filter_map(|oid| commits_by_oid.get(oid.as_str()))
        .any(|commit| {
            commit.parent_count > 1
                || commit.shallow_boundary
                || commit.changes.iter().any(|change| {
                    commit.anchored_ordinals.contains(&change.ordinal)
                        && (change.status.starts_with('R') || change.status.starts_with('C'))
                })
        });
    let shallow_history = target
        .warnings
        .iter()
        .any(|warning| warning.contains("local history is shallow"));
    let verified_introduction = introduction.as_ref().ok().cloned();

    let introduction = match introduction {
        Err(reason) => SymbolFact::Unknown { reason },
        Ok(_) if cache_gap => SymbolFact::Unknown {
            reason: "symbol history extends beyond published cache coverage".to_owned(),
        },
        Ok(_) if missing_objects => SymbolFact::Unknown {
            reason: "local Git objects needed to confirm the introduction are missing".to_owned(),
        },
        Ok(_) if shallow_history || uncertain_lineage => SymbolFact::Unknown {
            reason: "shallow, merge, or path-move history makes symbol lineage uncertain"
                .to_owned(),
        },
        Ok(oid) if !eligible_revisions.is_none_or(|eligible| eligible.contains(&oid)) => {
            SymbolFact::Unknown {
                reason: "symbol introduction is outside the current query scope".to_owned(),
            }
        }
        Ok(oid) => match commits_by_oid.get(oid.as_str()) {
            Some(commit) => SymbolFact::Known {
                commit_oid: oid,
                subject: commit.subject.clone(),
            },
            None => SymbolFact::Unknown {
                reason: "symbol introduction is not present in cached path history".to_owned(),
            },
        },
    };

    let anchor_line_attribution = match &target.blame {
        None => SymbolFact::Unknown {
            reason: "Git could not attribute the symbol anchor line".to_owned(),
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

    let mut priorities = HunkPriorities::new();
    if with_patch {
        let mut historical_start = anchor_line(&target.anchor) as i64;
        let mut historical_end = target.symbol_end.unwrap_or(historical_start as usize) as i64;
        for commit in &commits {
            let hunks = session.history_hunks(&commit.oid)?;
            if trace_revisions.contains(commit.oid.as_str()) {
                for hunk in &hunks {
                    if commit.anchored_ordinals.contains(&hunk.change_ordinal)
                        && retrieval::hunk_overlaps_symbol(
                            hunk,
                            historical_start.min(historical_end),
                            historical_start.max(historical_end),
                        )
                    {
                        priorities
                            .entry(commit.oid.clone())
                            .or_default()
                            .insert(hunk.id(), 0);
                    }
                }
            }
            retrieval::trace_line(commit, &hunks, &mut historical_start);
            retrieval::trace_line(commit, &hunks, &mut historical_end);
        }
    }

    let mut entries = Vec::new();
    for oid in &revisions {
        if verified_introduction.as_deref() == Some(oid.as_str()) || !reachable.contains(oid) {
            continue;
        }
        if !eligible_revisions.is_none_or(|eligible| eligible.contains(oid)) {
            continue;
        }
        let Some(commit) = commits_by_oid.get(oid.as_str()) else {
            continue;
        };
        let explanation = explanation_strength(&commit.subject, &commit.body);
        let cochanged = cochanged_paths(commit, &target.path);
        let mut basis = vec!["Git range tracing identifies a change to this symbol".to_owned()];
        if explanation > 0 {
            basis.push("explanatory commit body".to_owned());
        }
        if cochanged > 0 {
            basis.push(format!("co-changed paths ({cochanged})"));
        }
        let confidence = if explanation >= 2 {
            Confidence::High
        } else if explanation > 0 {
            Confidence::Medium
        } else {
            Confidence::Low
        };
        entries.push(retrieval::Ranked {
            score: explanation as f64 * 8.0 + cochanged.min(5) as f64 * 1.5,
            commit_time: commit.commit_time,
            oid: commit.oid.clone(),
            value: Entry {
                subject: commit.subject.clone(),
                paths: commit.paths.clone(),
                confidence,
                basis,
                citations: vec![Citation::new(commit.oid.clone(), commit.subject.clone())],
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
                    line: anchor_line(&target.anchor),
                })),
                patch: None,
            }
        })
        .collect::<Vec<_>>();
    retrieval::assign_citations(&mut materials);

    let mut report = super::super::report(ReportKind::Why, materials, matched_count, limit);
    report.patch_mode = with_patch;
    report.symbol_summary = Some(SymbolSummary {
        target: anchor_description(&target.anchor),
        introduction,
        anchor_line_attribution,
    });
    report.notices.extend(target.warnings.iter().cloned());
    report.notices.push(REMOTE_CONTEXT_NOTICE.to_owned());
    if missing_objects {
        report.notices.push(
            "warning: local cache is missing Git objects; why material may be incomplete."
                .to_owned(),
        );
    }
    if with_patch {
        patch::attach_selected_patch_excerpts(session, &mut report, &priorities)?;
    }
    Ok(report)
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
