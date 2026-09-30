use std::collections::HashSet;

use crate::{app::AppError, cache::QuerySession, git::RegressionTarget};

use super::super::patch::{self, HunkPriorities};
use super::super::retrieval;
use super::super::{Citation, Confidence, Intent, Material, Report, ReportKind};

const BISECT_NOTICE: &str =
    "Regression suspects are historical candidates; they do not replace executable git bisect.";

pub(crate) fn run(
    session: &QuerySession,
    intent: &Intent,
    target: &RegressionTarget,
    reachable: &HashSet<String>,
    limit: usize,
    with_patch: bool,
) -> Result<Report, AppError> {
    let history = session.path_history(&target.path, reachable)?;
    let missing_objects = session.has_missing_objects(&history)?;
    let mut symbol_range = target
        .symbol_line
        .zip(target.symbol_end)
        .map(|(start, end)| (start as i64, end as i64));
    let mut priorities = HunkPriorities::new();

    let rename_boundary = history.iter().any(has_path_boundary);
    let history_len = history.len();
    let mut ranked = Vec::new();
    for (history_index, commit) in history.into_iter().enumerate() {
        let hunks = session.history_hunks(&commit.oid)?;
        // Material and excerpts see the same symbol range before tracing to the parent.
        let mut overlaps_symbol = false;
        let mut symptom_hunk = false;
        for hunk in hunks
            .iter()
            .filter(|hunk| commit.anchored_ordinals.contains(&hunk.change_ordinal))
        {
            let symptom_match =
                count_term_hits(intent.terms(), &String::from_utf8_lossy(&hunk.text)) > 0;
            let symbol_match = symbol_range.is_some_and(|(start, end)| {
                retrieval::hunk_overlaps_symbol(hunk, start.min(end), start.max(end))
            });
            symptom_hunk |= symptom_match;
            overlaps_symbol |= symbol_match;
            if with_patch && (symptom_match || symbol_match) {
                let priority = match (symptom_match, symbol_match) {
                    (true, true) => 0,
                    (false, true) => 1,
                    (true, false) => 2,
                    (false, false) => unreachable!(),
                };
                priorities
                    .entry(commit.oid.clone())
                    .or_default()
                    .insert(hunk.id(), priority);
            }
        }
        let symbol_match = if let Some((start, end)) = &mut symbol_range {
            let start_changed = retrieval::trace_line(&commit, &hunks, start).is_some();
            let end_changed = retrieval::trace_line(&commit, &hunks, end).is_some();
            overlaps_symbol || start_changed || end_changed
        } else {
            false
        };
        if symbol_range.is_some() && !symbol_match {
            continue;
        }
        let message = format!("{}\n{}", commit.subject, commit.body);
        let lexical_hits = count_term_hits(intent.terms(), &message);
        let hunk_hits = usize::from(symptom_hunk);
        let test_paths = commit
            .paths
            .iter()
            .filter(|path| is_test_path(path))
            .count();
        let temporal = temporal_score(history_index, history_len);
        let rename_boundary = has_path_boundary(&commit);
        if lexical_hits == 0 && hunk_hits == 0 && !symbol_match && test_paths == 0 {
            continue;
        }
        let score = lexical_hits as f64 * 14.0
            + hunk_hits as f64 * 8.0
            + if symbol_match { 12.0 } else { 0.0 }
            + test_paths as f64 * 4.0
            + temporal * 3.0;
        let confidence = if lexical_hits > 0 && hunk_hits > 0 && !missing_objects {
            Confidence::High
        } else if lexical_hits > 0 || hunk_hits > 0 || symbol_match || test_paths > 0 {
            Confidence::Medium
        } else {
            Confidence::Low
        };
        let mut basis = vec!["affected path history".to_owned()];
        if target.good_revision.is_some() {
            basis.push("candidate is within the pinned good..bad range".to_owned());
        } else {
            basis.push("candidate is reachable from the pinned bad revision".to_owned());
        }
        if symbol_match {
            if let Some(symbol) = &target.symbol {
                basis.push(format!("symbol/hunk overlap for {symbol}"));
            } else {
                basis.push("symbol/hunk overlap".to_owned());
            }
        }
        if lexical_hits > 0 {
            basis.push(format!(
                "symptom lexical match ({lexical_hits}/{} terms)",
                intent.terms().len()
            ));
        }
        if hunk_hits > 0 {
            basis.push("diff hunk overlaps the symptom".to_owned());
        }
        if test_paths > 0 {
            basis.push("test-history signal".to_owned());
        }
        if temporal > 0.0 {
            basis.push("temporal position in requested range".to_owned());
        }
        if rename_boundary {
            basis.push("move/rename boundary".to_owned());
        }
        if commit.parent_count > 1 {
            basis.push("merge boundary".to_owned());
        }
        if missing_objects {
            basis.push("missing local object prevented complete diff corroboration".to_owned());
        }
        ranked.push(retrieval::Ranked {
            score,
            commit_time: commit.commit_time,
            oid: commit.oid.clone(),
            value: Material {
                subject: commit.subject.clone(),
                paths: commit.paths.clone(),
                confidence,
                basis,
                citations: vec![Citation::new(commit.oid, commit.subject)],
                detail: None,
                patch: None,
            },
        });
    }

    retrieval::sort(&mut ranked);
    let matched_count = ranked.len();
    let mut materials = ranked
        .into_iter()
        .take(limit)
        .map(|candidate| candidate.value)
        .collect::<Vec<_>>();
    retrieval::assign_citations(&mut materials);

    let mut report = super::super::report(ReportKind::Regression, materials, matched_count, limit);
    report.warnings.extend(target.warnings.iter().cloned());
    if rename_boundary {
        report.warnings.push(
            "warning: path history crossed a move/rename boundary; suspects may be incomplete."
                .to_owned(),
        );
    }
    if missing_objects {
        report.warnings.push(
            "warning: local cache or target history is missing Git objects; regression material may be incomplete."
                .to_owned(),
        );
    }
    report.notices.push(BISECT_NOTICE.to_owned());
    if with_patch {
        patch::attach_selected_patch_excerpts(session, &mut report, &priorities)?;
    }
    Ok(report)
}

fn count_term_hits(terms: &[String], text: &str) -> usize {
    let tokens = retrieval::tokenize(text);
    terms
        .iter()
        .filter(|term| tokens.iter().any(|token| token == *term))
        .count()
}

fn has_path_boundary(commit: &retrieval::HistoryCommit) -> bool {
    commit
        .changes
        .iter()
        .filter(|change| commit.anchored_ordinals.contains(&change.ordinal))
        .any(|change| change.status.starts_with('R') || change.status.starts_with('C'))
}
fn temporal_score(index: usize, history_len: usize) -> f64 {
    if history_len == 0 {
        return 0.0;
    }
    (index + 1) as f64 / (history_len as f64 + 1.0)
}

fn is_test_path(path: &[u8]) -> bool {
    let path = retrieval::normalize_path(path);
    let mut parts = path.rsplit('/');
    let name = parts.next().unwrap_or_default();
    if parts.any(|part| matches!(part, "test" | "tests" | "__tests__" | "spec")) {
        return true;
    }
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    stem.starts_with("test_")
        || stem.starts_with("test-")
        || stem.ends_with("_test")
        || stem.ends_with("_spec")
        || name.contains(".test.")
        || name.contains(".spec.")
}
