use rusqlite::Connection;

use crate::app::AppError;

use super::super::provenance::{revert_reason, stated_reason, stated_retry};
use super::super::retrieval;
use super::super::{Citation, Confidence, Detail, Failure, Intent, Material, Report, ReportKind};
use super::{confidence, empty};

/// A candidate must share at least one intent term before its paths count as evidence.
const MIN_SHARED_TERMS: usize = 1;
/// History recording the revert itself is the strongest failure evidence there is.
const REVERT_WEIGHT: f64 = 6.0;
/// A later correction of the abandoned paths explains how the approach moved on.
const FOLLOW_UP_WEIGHT: f64 = 3.0;

pub(crate) fn run(
    connection: &Connection,
    intent: &Intent,
    limit: usize,
) -> Result<Report, AppError> {
    let Some(pool) = retrieval::pool(connection, intent, limit)? else {
        return Ok(empty(ReportKind::Failures));
    };
    let reverts = retrieval::reverts(connection)?;

    // Which commit history records as reverting each candidate. Computing this once keeps
    // the fold and the citation consistent.
    let mut linked = Vec::with_capacity(pool.candidates.len());
    for candidate in &pool.candidates {
        linked.push(
            retrieval::link(connection, &reverts, candidate)?.map(|revert| revert.oid.clone()),
        );
    }

    let mut ranked = Vec::new();
    for (index, candidate) in pool.candidates.iter().enumerate() {
        if candidate.signals.shared_terms() < MIN_SHARED_TERMS {
            continue;
        }
        // A revert told by the abandoned change it undid is not a second result.
        let fold_into = linked
            .iter()
            .enumerate()
            .find(|(other, reverted)| {
                *other != index && reverted.as_deref() == Some(&candidate.oid)
            })
            .map(|(other, _)| other);
        if fold_into.is_some() {
            continue;
        }

        let mut score = candidate.signals.score();
        let mut basis = Vec::new();
        candidate.signals.describe(&mut basis);
        let mut citations = vec![Citation::new(
            candidate.oid.clone(),
            candidate.subject.clone(),
        )];
        let mut confidence = confidence(&candidate.signals);
        let mut reason = if reverts.is_revert(&candidate.oid) {
            revert_reason(&candidate.subject, &candidate.body)
        } else {
            stated_reason(&candidate.subject, &candidate.body)
        };

        if let Some(revert) = retrieval::link(connection, &reverts, candidate)? {
            score += REVERT_WEIGHT;
            basis.push("recorded revert".to_owned());
            citations.push(
                Citation::new(revert.oid.clone(), revert.subject.clone())
                    .noting("reverts this change"),
            );
            reason = revert_reason(&revert.subject, &revert.body).or(reason);
            confidence = Confidence::High;
        }

        // Look for a correction after the revert, or after the change when none is recorded.
        let after = linked[index].as_deref().unwrap_or(&candidate.oid);
        let mut retry = stated_retry(&candidate.body);
        if let Some((oid, subject)) =
            retrieval::corrective_follow_up(connection, after, &candidate.paths)?
        {
            score += FOLLOW_UP_WEIGHT;
            basis.push("corrective follow-up".to_owned());
            let follow_up = retrieval::commit_text(connection, &oid)?;
            retry = follow_up
                .map(|text| format!("{}\n{}", text.0, text.1))
                .and_then(|text| stated_retry(&text))
                .or(retry);
            citations.push(Citation::new(oid, subject).noting("follow-up"));
            confidence = Confidence::High;
        }
        if retry.is_none()
            && let Some(revert) = retrieval::link(connection, &reverts, candidate)?
        {
            retry = stated_retry(&revert.body);
        }

        ranked.push(retrieval::Ranked {
            score,
            commit_time: candidate.commit_time,
            oid: candidate.oid.clone(),
            value: (index, basis, citations, reason, retry, confidence),
        });
    }
    retrieval::sort(&mut ranked);

    let mut materials = ranked
        .into_iter()
        .take(limit)
        .map(|ranked| {
            let (index, basis, citations, reason, retry, confidence) = ranked.value;
            let candidate = &pool.candidates[index];
            Material {
                subject: candidate.subject.clone(),
                paths: candidate.paths.clone(),
                confidence,
                basis,
                citations,
                detail: Some(Detail::Failure(Failure { reason, retry })),
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
