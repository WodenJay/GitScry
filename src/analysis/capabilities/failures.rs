use rusqlite::Connection;

use crate::app::AppError;

use super::super::provenance::{revert_reason, stated_retry};
use super::super::retrieval;
use super::super::{Citation, Confidence, Detail, Failure, Intent, Material, Report, ReportKind};
use super::lexical_confidence;

/// One assembled failure result, before the report decides which survive the limit.
struct Entry {
    index: usize,
    basis: Vec<String>,
    citations: Vec<Citation>,
    reason: Option<String>,
    retry: Option<String>,
    confidence: Confidence,
}

/// A candidate must share at least one intent term before its paths count as material.
const MIN_SHARED_TERMS: usize = 1;
/// History recording the revert itself is the strongest failure material there is.
const REVERT_WEIGHT: f64 = 6.0;
/// A later correction of the abandoned paths explains how the approach moved on.
const FOLLOW_UP_WEIGHT: f64 = 3.0;

pub(crate) fn run(
    connection: &Connection,
    intent: &Intent,
    limit: usize,
) -> Result<Report, AppError> {
    let Some(pool) = retrieval::pool(connection, intent, limit)? else {
        return Ok(super::super::empty_report(ReportKind::Failures));
    };
    let reverts = retrieval::reverts(connection)?;

    // Which commit history records as reverting each candidate. Computing this once keeps
    // the fold and the citation consistent.
    let mut linked = Vec::with_capacity(pool.candidates.len());
    for candidate in &pool.candidates {
        linked.push(retrieval::link(connection, &reverts, candidate)?);
    }

    let mut ranked = Vec::new();
    for (index, candidate) in pool.candidates.iter().enumerate() {
        if candidate.signals.shared_terms() < MIN_SHARED_TERMS {
            continue;
        }
        let linked_revert = linked[index];
        let is_revert = reverts.target_of(&candidate.oid).is_some();
        // A revert told by the abandoned change it undid is not a second result.
        if is_revert
            && linked.iter().enumerate().any(|(other, reverted)| {
                other != index && reverted.map(|revert| revert.oid.as_str()) == Some(&candidate.oid)
            })
        {
            continue;
        }
        // Failed-approach material is an abandoned change, or the revert that abandoned it.
        // Work no revert undid, and a revert that resolves to nothing in this repository, is
        // not a failed approach however much its text resembles one.
        if linked_revert.is_none() && !is_revert {
            continue;
        }

        let mut score = candidate.signals.identified_score();
        let mut basis = Vec::new();
        candidate.signals.describe_identified(&mut basis);
        let mut citations = vec![Citation::new(
            candidate.oid.clone(),
            candidate.subject.clone(),
        )];
        // Only a resolved revert states a failure reason. An abandoned change's own message
        // is not used, because GitScry never infers why work was undone.
        let mut reason = None;
        let mut retry = None;
        let mut follow_up = false;

        // The message that recorded the abandonment: the reverting commit for abandoned
        // work, or the revert's own message when that is the subject.
        let recording = linked_revert.or_else(|| reverts.resolved_revert(&candidate.oid));
        if let Some(revert) = recording {
            let abandoned_by_name = linked_revert.is_some();
            if abandoned_by_name {
                score += REVERT_WEIGHT;
                basis.push("recorded revert".to_owned());
                citations.push(
                    Citation::new(revert.oid.clone(), revert.subject.clone())
                        .noting("reverts this change"),
                );
            }
            reason = revert_reason(&revert.subject, &revert.body);
            retry = stated_retry(&revert.body);
            if let Some((oid, subject, body)) =
                retrieval::corrective_follow_up(connection, &revert.oid, &candidate.path_keys)?
            {
                score += FOLLOW_UP_WEIGHT;
                basis.push("corrective follow-up".to_owned());
                retry = stated_retry(&format!("{subject}\n{body}")).or(retry);
                citations.push(Citation::new(oid, subject).noting("follow-up"));
                follow_up = true;
            }
        }

        // Confidence reflects what was actually recovered, not that a link exists.
        let confidence = if reason.is_some() && follow_up {
            Confidence::High
        } else if reason.is_some() {
            Confidence::Medium
        } else {
            lexical_confidence(&candidate.signals)
        };

        ranked.push(retrieval::Ranked {
            score,
            commit_time: candidate.commit_time,
            oid: candidate.oid.clone(),
            value: Entry {
                index,
                basis,
                citations,
                reason,
                retry,
                confidence,
            },
        });
    }
    retrieval::sort(&mut ranked);

    let mut materials = ranked
        .into_iter()
        .take(limit)
        .map(|ranked| {
            let entry = ranked.value;
            let candidate = &pool.candidates[entry.index];
            Material {
                subject: candidate.subject.clone(),
                paths: candidate.paths.clone(),
                confidence: entry.confidence,
                basis: entry.basis,
                citations: entry.citations,
                detail: Some(Detail::Failure(Failure {
                    reason: entry.reason,
                    retry: entry.retry,
                })),
            }
        })
        .collect::<Vec<_>>();
    retrieval::assign_citations(&mut materials);

    Ok(super::super::report(
        ReportKind::Failures,
        materials,
        pool.matched_count,
        limit,
    ))
}
