use std::collections::{HashMap, HashSet};

use rusqlite::Connection;

use crate::{
    app::AppError,
    git::{DeletedLine, TraceFixTarget},
};

use super::super::provenance;
use super::super::retrieval;
use super::super::{Citation, Confidence, Detail, Material, Report, ReportKind, TraceFixDetail};

struct Evidence {
    oid: String,
    subject: String,
    commit_time: i64,
    parent_count: usize,
    boundary: bool,
    moved: bool,
    shared_reference: Option<String>,
    failure: Option<(String, String)>,
    movement: Option<(String, String)>,
    paths: Vec<Vec<u8>>,
    lines: Vec<usize>,
}

pub(crate) fn run(
    connection: &Connection,
    target: &TraceFixTarget,
    reachable: &HashSet<String>,
    limit: usize,
) -> Result<Report, AppError> {
    let histories = candidate_history(connection, &target.deleted_lines, reachable)?;
    let references = message_references(&target.fix.message);
    let pre_fix = target
        .parent
        .as_deref()
        .map(|parent| retrieval::ancestors(connection, parent))
        .transpose()?
        .unwrap_or_default();
    let evidence = collect_evidence(
        &target.deleted_lines,
        target.shallow,
        &histories,
        &references,
        &pre_fix,
        &target.fix.oid,
    );
    let (fix_subject, fix_body) = retrieval::message_parts(&target.fix.message);
    let observed_failure = provenance::stated_reason(&fix_subject, &fix_body)
        .or_else(|| provenance::revert_reason(&fix_subject, &fix_body))
        .is_some();

    let mut ranked = evidence
        .into_iter()
        .map(|evidence| {
            let confidence = if evidence.boundary {
                Confidence::Low
            } else if evidence.moved || evidence.parent_count > 1 || !observed_failure {
                Confidence::Medium
            } else {
                Confidence::High
            };
            let mut basis =
                vec!["fix-parent deleted-line blame identifies this introducing change".to_owned()];
            if evidence.lines.len() > 1 {
                basis.push(format!("deleted lines: {}", evidence.lines.len()));
            }
            if observed_failure {
                basis.push("fix commit message records observed failure context".to_owned());
            } else {
                basis.push(
                    "observed failure context is unavailable from local commit messages".to_owned(),
                );
            }
            if !references.is_empty() {
                basis.push(format!("fix message references: {}", references.join(", ")));
            }
            if let Some(reference) = evidence.shared_reference {
                basis.push(format!(
                    "message reference {reference} corroborates the lineage"
                ));
            }
            if let Some((_, _)) = &evidence.failure {
                basis.push("observed failure commit shares a fix message reference".to_owned());
            }
            if evidence.moved {
                basis.push(
                    "code movement: anchored path history followed a rename or copy".to_owned(),
                );
            }
            if evidence.parent_count > 1 {
                basis.push("merge lineage uses the ordered first parent".to_owned());
            }
            if evidence.boundary {
                basis.push("shallow history boundary".to_owned());
            }
            let mut citations = vec![
                Citation::new(evidence.oid.clone(), evidence.subject.clone())
                    .noting("introducing change"),
            ];
            if let Some((failure_oid, failure_subject)) = evidence.failure.clone() {
                citations
                    .push(Citation::new(failure_oid, failure_subject).noting("observed failure"));
            }
            if let Some((movement_oid, movement_subject)) = evidence.movement.clone() {
                citations
                    .push(Citation::new(movement_oid, movement_subject).noting("code movement"));
            }
            let fix_citation = Citation::new(target.fix.oid.clone(), fix_subject.clone());
            citations.push(if observed_failure {
                fix_citation.noting("observed failure and fix")
            } else {
                fix_citation.noting("fix")
            });
            let first_line = evidence.lines.first().copied();
            retrieval::Ranked {
                score: if evidence.boundary { 70.0 } else { 90.0 },
                commit_time: evidence.commit_time,
                oid: evidence.oid.clone(),
                value: Material {
                    subject: if evidence.subject.is_empty() {
                        "Introducing commit (subject unavailable)".to_owned()
                    } else {
                        evidence.subject
                    },
                    paths: evidence.paths,
                    confidence,
                    basis,
                    citations,
                    detail: Some(Detail::TraceFix(TraceFixDetail {
                        role: "introducing change",
                        fix_revision: target.revision.clone(),
                        parent_revision: target.parent.clone(),
                        line: first_line,
                    })),
                },
            }
        })
        .collect::<Vec<_>>();

    retrieval::sort(&mut ranked);
    let matched_count = ranked.len();
    let mut materials = ranked
        .into_iter()
        .take(limit)
        .map(|ranked| ranked.value)
        .collect::<Vec<_>>();
    retrieval::assign_citations(&mut materials);
    let mut report = super::super::report(ReportKind::TraceFix, materials, matched_count, limit);
    report.notices.extend(target.warnings.iter().cloned());
    report
        .notices
        .push("Remote context unavailable from local history.".to_owned());
    if !target.deleted_lines.is_empty() && matched_count == 0 {
        report.notices.push(
            "warning: no introducing commit is available in the default-branch cache.".to_owned(),
        );
    }
    Ok(report)
}

pub(crate) fn without_cache(target: &TraceFixTarget, limit: usize) -> Result<Report, AppError> {
    let (subject, _) = retrieval::message_parts(&target.fix.message);
    let mut paths = Vec::new();
    for deleted in &target.deleted_lines {
        if !paths.iter().any(|path| path == &deleted.path) {
            paths.push(deleted.path.clone());
        }
    }
    let deleted_line = target.deleted_lines.first().map(|deleted| deleted.line);
    let mut report = super::super::report(
        ReportKind::TraceFix,
        vec![Material {
            subject,
            paths,
            confidence: Confidence::Low,
            basis: vec![
                "fix-parent deleted-line facts are available in memory".to_owned(),
                "introducing candidates are unavailable without a default-branch cache".to_owned(),
            ],
            citations: vec![
                Citation::new(target.fix.oid.clone(), "".to_owned()).noting("fix context"),
            ],
            detail: Some(Detail::TraceFix(TraceFixDetail {
                role: "fix context",
                fix_revision: target.revision.clone(),
                parent_revision: target.parent.clone(),
                line: deleted_line,
            })),
        }],
        1,
        limit,
    );
    retrieval::assign_citations(&mut report.materials);
    report.notices.extend(target.warnings.iter().cloned());
    report
        .notices
        .push("Remote context unavailable from local history.".to_owned());
    Ok(report)
}

fn candidate_history(
    connection: &Connection,
    deleted_lines: &[DeletedLine],
    reachable: &HashSet<String>,
) -> Result<HashMap<String, retrieval::HistoryCommit>, AppError> {
    let mut histories = HashMap::new();
    let mut paths = Vec::<Vec<u8>>::new();
    for deleted in deleted_lines {
        if !paths.iter().any(|path| path == &deleted.path) {
            paths.push(deleted.path.clone());
        }
    }
    for path in paths {
        for commit in retrieval::path_history(connection, &path, reachable)? {
            histories.entry(commit.oid.clone()).or_insert(commit);
        }
    }
    Ok(histories)
}

fn collect_evidence(
    deleted_lines: &[DeletedLine],
    shallow: bool,
    histories: &HashMap<String, retrieval::HistoryCommit>,
    references: &[String],
    pre_fix: &HashSet<String>,
    fix_oid: &str,
) -> Vec<Evidence> {
    let mut by_oid = HashMap::<String, Evidence>::new();
    for deleted in deleted_lines {
        let Some(blame) = &deleted.blame else {
            continue;
        };
        let Some(history) = histories.get(&blame.oid) else {
            continue;
        };
        let candidate_references = message_references_from_parts(&history.subject, &history.body);
        let shared_reference = references
            .iter()
            .find(|reference| {
                candidate_references
                    .iter()
                    .any(|candidate| candidate == *reference)
            })
            .cloned();
        let failure = histories
            .values()
            .filter(|candidate| {
                pre_fix.contains(&candidate.oid)
                    && candidate.position > history.position
                    && candidate.oid != *fix_oid
                    && candidate.oid != blame.oid
            })
            .filter_map(|candidate| {
                let candidate_references =
                    message_references_from_parts(&candidate.subject, &candidate.body);
                let reference = references.iter().find(|reference| {
                    candidate_references
                        .iter()
                        .any(|candidate| candidate == *reference)
                })?;
                Some((
                    candidate.position,
                    candidate.oid.clone(),
                    candidate.subject.clone(),
                    reference,
                ))
            })
            .max_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)))
            .map(|(_, oid, subject, _)| (oid, subject));
        let movement = histories
            .values()
            .filter(|candidate| candidate.oid != blame.oid && anchored_moved(candidate))
            .filter(|candidate| pre_fix.is_empty() || pre_fix.contains(&candidate.oid))
            .max_by_key(|candidate| candidate.position)
            .map(|candidate| (candidate.oid.clone(), candidate.subject.clone()));
        let mut paths = anchored_paths(history);
        if let Some((movement_oid, _)) = &movement
            && let Some(movement_history) = histories.get(movement_oid)
        {
            for path in anchored_paths(movement_history) {
                if !paths.iter().any(|existing| existing == &path) {
                    paths.push(path);
                }
            }
        }
        let moved = anchored_moved(history) || movement.is_some();
        let evidence = by_oid.entry(blame.oid.clone()).or_insert_with(|| Evidence {
            oid: blame.oid.clone(),
            subject: history.subject.clone(),
            commit_time: history.commit_time,
            parent_count: history.parent_count,
            boundary: shallow && blame.boundary,
            moved,
            shared_reference: shared_reference.clone(),
            failure: failure.clone(),
            movement: movement.clone(),
            paths,
            lines: Vec::new(),
        });
        if let Some(reference) = shared_reference {
            evidence.shared_reference = Some(reference);
        }
        if !evidence.paths.iter().any(|path| path == &deleted.path) {
            evidence.paths.push(deleted.path.clone());
        }
        evidence.lines.push(deleted.line);
        evidence.boundary |= shallow && blame.boundary;
    }
    by_oid.into_values().collect()
}

fn anchored_moved(history: &retrieval::HistoryCommit) -> bool {
    history.changes.iter().any(|change| {
        history.anchored_ordinals.contains(&change.ordinal)
            && (change.status.starts_with('R') || change.status.starts_with('C'))
    })
}

fn anchored_paths(history: &retrieval::HistoryCommit) -> Vec<Vec<u8>> {
    let mut paths = Vec::new();
    for change in &history.changes {
        if !history.anchored_ordinals.contains(&change.ordinal) {
            continue;
        }
        for path in [&change.old_path, &change.new_path].into_iter().flatten() {
            if !paths.iter().any(|existing| existing == path) {
                paths.push(path.clone());
            }
        }
    }
    paths
}

fn message_references(message: &[u8]) -> Vec<String> {
    let (subject, body) = retrieval::message_parts(message);
    message_references_from_parts(&subject, &body)
}

fn message_references_from_parts(subject: &str, body: &str) -> Vec<String> {
    let mut references = subject
        .split_whitespace()
        .chain(body.split_whitespace())
        .filter_map(normalize_reference)
        .collect::<Vec<_>>();
    references.sort();
    references.dedup();
    references
}

fn normalize_reference(word: &str) -> Option<String> {
    let trimmed = word.trim_matches(|character: char| {
        !character.is_ascii_alphanumeric() && character != '#' && character != '-'
    });
    let number = trimmed.strip_prefix('#')?;
    (!number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| trimmed.to_owned())
}
