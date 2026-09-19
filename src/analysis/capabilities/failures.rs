use rusqlite::Connection;

use crate::app::AppError;

use super::super::provenance::{stated_reason, stated_retry};
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

    let mut ranked = Vec::new();
    for candidate in &pool.candidates {
        if candidate.signals.shared_terms() < MIN_SHARED_TERMS {
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
        let mut reason = stated_reason(&candidate.subject, &candidate.body);

        if let Some(revert) = reverts.reverting(&candidate.oid) {
            score += REVERT_WEIGHT;
            basis.push("recorded revert".to_owned());
            citations.push(
                Citation::new(revert.oid.clone(), revert.subject.clone())
                    .noting("reverts this change"),
            );
            reason = stated_reason(&revert.subject, &revert.body).or(reason);
            confidence = Confidence::High;
        }

        let mut retry = stated_retry(&candidate.body);
        if let Some((oid, subject)) =
            retrieval::corrective_follow_up(connection, candidate.commit_time, &candidate.paths)?
        {
            score += FOLLOW_UP_WEIGHT;
            basis.push("corrective follow-up".to_owned());
            citations.push(Citation::new(oid.clone(), subject).noting("follow-up"));
            retry = retrieval::commit_text(connection, &oid)?
                .map(|(subject, body)| format!("{subject}\n{body}"))
                .and_then(|text| stated_retry(&text))
                .or(retry);
            confidence = Confidence::High;
        }

        ranked.push(retrieval::Ranked {
            score,
            commit_time: candidate.commit_time,
            oid: candidate.oid.clone(),
            value: (candidate, basis, citations, reason, retry, confidence),
        });
    }
    retrieval::sort(&mut ranked);

    let mut materials = ranked
        .into_iter()
        .take(limit)
        .map(|ranked| {
            let (candidate, basis, citations, reason, retry, confidence) = ranked.value;
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
