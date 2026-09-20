use std::collections::{HashMap, HashSet};

use rusqlite::Connection;

use crate::{
    app::AppError,
    git::{Change, RegressionTarget, Snapshot},
};

use super::super::retrieval;
use super::super::{Citation, Confidence, Intent, Material, Report, ReportKind};

const BISECT_NOTICE: &str =
    "Regression suspects are historical candidates; they do not replace executable git bisect.";

pub(crate) fn run(
    connection: &Connection,
    intent: &Intent,
    target: &RegressionTarget,
    direct_history: Option<&Snapshot>,
    limit: usize,
) -> Result<Report, AppError> {
    let history = match direct_history {
        Some(snapshot) => direct_path_history(snapshot, &target.path, &target.range),
        None => retrieval::path_history(connection, &target.path, &target.range)?,
    };
    let missing_objects = match direct_history {
        Some(snapshot) => snapshot_missing_objects(snapshot, &history),
        None => retrieval::has_missing_objects(connection, &history)?,
    };
    let symbol_matches = match (target.symbol_line, target.symbol_end) {
        (Some(symbol_line), Some(symbol_end)) => Some(symbol_matches(
            connection,
            direct_history,
            &history,
            symbol_line,
            symbol_end,
        )?),
        _ => None,
    };

    let rename_boundary = history.iter().any(has_path_boundary);
    let history_len = history.len();
    let mut ranked = Vec::new();
    for (history_index, commit) in history.into_iter().enumerate() {
        if let Some(matches) = &symbol_matches
            && !matches.contains(&commit.oid)
        {
            continue;
        }
        let hunks = commit_hunks(connection, direct_history, &commit.oid)?;
        let (lexical_hits, hunk_hits) = symptom_hits(intent, &commit, &hunks);
        let test_paths = commit
            .paths
            .iter()
            .filter(|path| is_test_path(path))
            .count();
        let symbol_match = symbol_matches
            .as_ref()
            .is_some_and(|matches| matches.contains(&commit.oid));
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
    Ok(report)
}

fn symptom_hits(
    intent: &Intent,
    commit: &retrieval::HistoryCommit,
    hunks: &[retrieval::HistoryHunk],
) -> (usize, usize) {
    let message = format!("{}\n{}", commit.subject, commit.body);
    let lexical_hits = count_term_hits(intent.terms(), &message);
    let anchored = commit
        .anchored_ordinals
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    let hunk_text = hunks
        .iter()
        .filter(|hunk| anchored.contains(&hunk.change_ordinal))
        .map(|hunk| String::from_utf8_lossy(&hunk.text))
        .collect::<Vec<_>>()
        .join("\n");
    let hunk_hits =
        usize::from(!hunk_text.is_empty() && count_term_hits(intent.terms(), &hunk_text) > 0);
    (lexical_hits, hunk_hits)
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
fn symbol_matches(
    connection: &Connection,
    direct_history: Option<&Snapshot>,
    history: &[retrieval::HistoryCommit],
    symbol_line: usize,
    symbol_end: usize,
) -> Result<HashSet<String>, AppError> {
    let mut line = symbol_line as i64;
    let mut end_line = symbol_end as i64;
    let mut matches = HashSet::new();
    for commit in history {
        let hunks = commit_hunks(connection, direct_history, &commit.oid)?;
        let overlaps_symbol = hunks.iter().any(|hunk| {
            commit.anchored_ordinals.contains(&hunk.change_ordinal)
                && hunk_overlaps_symbol(hunk, line.min(end_line), line.max(end_line))
        });
        let line_changed = trace_line(commit, &hunks, &mut line);
        let end_changed = trace_line(commit, &hunks, &mut end_line);
        if overlaps_symbol || line_changed || end_changed {
            matches.insert(commit.oid.clone());
        }
    }
    Ok(matches)
}

fn hunk_overlaps_symbol(hunk: &retrieval::HistoryHunk, start: i64, end: i64) -> bool {
    let mut line = hunk.new_start;
    for diff_line in hunk.text.split_inclusive(|byte| *byte == b'\n') {
        let Some(marker) = diff_line.first() else {
            continue;
        };
        match marker {
            b'+' => {
                if (start..=end).contains(&line) {
                    return true;
                }
                line += 1;
            }
            b'-' => {
                if (start..=end).contains(&line) {
                    return true;
                }
            }
            b' ' => line += 1,
            b'\\' | b'@' => {}
            _ => {}
        }
    }
    false
}

fn trace_line(
    commit: &retrieval::HistoryCommit,
    hunks: &[retrieval::HistoryHunk],
    line: &mut i64,
) -> bool {
    let relevant = commit
        .anchored_ordinals
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    let mut ordered = hunks
        .iter()
        .filter(|hunk| relevant.contains(&hunk.change_ordinal))
        .collect::<Vec<_>>();
    ordered.sort_by_key(|hunk| std::cmp::Reverse(hunk.new_start));
    for hunk in ordered {
        let new_end = hunk.new_start + hunk.new_lines - 1;
        if *line > new_end {
            *line += hunk.old_lines - hunk.new_lines;
            continue;
        }
        if *line < hunk.new_start || hunk.new_lines == 0 {
            continue;
        }
        let mut old_line = hunk.old_start;
        let mut new_line = hunk.new_start;
        let mut deleted_start = None;
        let mut direct = false;
        for diff_line in hunk.text.split_inclusive(|byte| *byte == b'\n') {
            let Some(marker) = diff_line.first() else {
                continue;
            };
            match marker {
                b'+' => {
                    if new_line == *line {
                        direct = true;
                        *line = deleted_start.unwrap_or(old_line);
                    }
                    new_line += 1;
                }
                b'-' => {
                    deleted_start.get_or_insert(old_line);
                    old_line += 1;
                }
                b' ' => {
                    if new_line == *line {
                        *line = old_line;
                    }
                    old_line += 1;
                    new_line += 1;
                    deleted_start = None;
                }
                b'\\' | b'@' => {}
                _ => {}
            }
        }
        return direct;
    }
    false
}

fn commit_hunks(
    connection: &Connection,
    direct_history: Option<&Snapshot>,
    oid: &str,
) -> Result<Vec<retrieval::HistoryHunk>, AppError> {
    match direct_history {
        Some(snapshot) => Ok(snapshot
            .hunks
            .iter()
            .filter(|hunk| hunk.commit_oid == oid)
            .map(|hunk| retrieval::HistoryHunk {
                change_ordinal: hunk.change_ordinal,
                old_start: hunk.old_start,
                old_lines: hunk.old_lines,
                new_start: hunk.new_start,
                new_lines: hunk.new_lines,
                text: hunk.text.clone(),
            })
            .collect()),
        None => retrieval::hunks(connection, oid),
    }
}

fn temporal_score(index: usize, history_len: usize) -> f64 {
    if history_len == 0 {
        return 0.0;
    }
    (history_len.saturating_sub(index)) as f64 / (history_len as f64 + 1.0)
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

fn snapshot_missing_objects(snapshot: &Snapshot, history: &[retrieval::HistoryCommit]) -> bool {
    let missing = snapshot.missing_objects.iter().collect::<HashSet<_>>();
    history.iter().any(|commit| {
        commit
            .changes
            .iter()
            .filter(|change| commit.anchored_ordinals.contains(&change.ordinal))
            .flat_map(|change| [&change.old_blob, &change.new_blob])
            .flatten()
            .any(|blob| missing.contains(blob))
    })
}

fn direct_path_history(
    snapshot: &Snapshot,
    path: &[u8],
    reachable: &HashSet<String>,
) -> Vec<retrieval::HistoryCommit> {
    let mut changes_by_path = HashMap::<Vec<u8>, Vec<&Change>>::new();
    let mut changes_by_commit = HashMap::<String, Vec<&Change>>::new();
    for change in &snapshot.changes {
        if !reachable.contains(&change.commit_oid) {
            continue;
        }
        for changed_path in [&change.old_path, &change.new_path].into_iter().flatten() {
            changes_by_path
                .entry(changed_path.clone())
                .or_default()
                .push(change);
        }
        changes_by_commit
            .entry(change.commit_oid.clone())
            .or_default()
            .push(change);
    }

    let mut pending = vec![path.to_vec()];
    let mut visited_paths = HashSet::new();
    let mut anchored = HashMap::<String, HashSet<i64>>::new();
    while let Some(current) = pending.pop() {
        if !visited_paths.insert(current.clone()) {
            continue;
        }
        for change in changes_by_path.get(&current).into_iter().flatten() {
            anchored
                .entry(change.commit_oid.clone())
                .or_default()
                .insert(change.ordinal);
            if (change.status.starts_with('R') || change.status.starts_with('C'))
                && change.new_path.as_deref() == Some(current.as_slice())
                && let Some(old_path) = &change.old_path
            {
                pending.push(old_path.clone());
            } else if (change.status.starts_with('R') || change.status.starts_with('C'))
                && change.old_path.as_deref() == Some(current.as_slice())
                && let Some(new_path) = &change.new_path
            {
                pending.push(new_path.clone());
            }
        }
    }

    let positions = snapshot
        .commits
        .iter()
        .enumerate()
        .map(|(position, commit)| (commit.oid.as_str(), position as i64))
        .collect::<HashMap<_, _>>();
    let commits = snapshot
        .commits
        .iter()
        .map(|commit| (commit.oid.as_str(), commit))
        .collect::<HashMap<_, _>>();
    let mut result = anchored
        .into_iter()
        .filter_map(|(oid, anchored_ordinals)| {
            let commit = commits.get(oid.as_str())?;
            let changes = changes_by_commit.get(&oid)?;
            let (subject, body) = retrieval::message_parts(&commit.message);
            let mut paths = Vec::new();
            let mut all_changes = Vec::new();
            for change in changes {
                for changed_path in [&change.old_path, &change.new_path].into_iter().flatten() {
                    if !paths.contains(changed_path) {
                        paths.push(changed_path.clone());
                    }
                }
                all_changes.push(retrieval::PathChange {
                    ordinal: change.ordinal,
                    status: change.status.clone(),
                    old_path: change.old_path.clone(),
                    new_path: change.new_path.clone(),
                    old_blob: change.old_blob.clone(),
                    new_blob: change.new_blob.clone(),
                });
            }
            all_changes.sort_by_key(|change| change.ordinal);
            Some(retrieval::HistoryCommit {
                position: *positions.get(oid.as_str())?,
                oid,
                commit_time: commit.time,
                subject,
                body,
                paths,
                changes: all_changes,
                anchored_ordinals: anchored_ordinals.into_iter().collect(),
                parent_count: commit.parents.len(),
            })
        })
        .collect::<Vec<_>>();
    result.sort_by(|left, right| {
        right
            .position
            .cmp(&left.position)
            .then_with(|| left.oid.cmp(&right.oid))
    });
    result
}
