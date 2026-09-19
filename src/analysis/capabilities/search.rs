use rusqlite::Connection;

use crate::app::AppError;

use super::super::retrieval;
use super::super::{Citation, Intent, Material, Report, ReportKind};
use super::lexical_confidence;

pub(crate) fn run(
    connection: &Connection,
    intent: &Intent,
    limit: usize,
) -> Result<Report, AppError> {
    let Some(pool) = retrieval::pool(connection, intent, limit)? else {
        return Ok(super::super::empty_report(ReportKind::Search));
    };

    let mut ranked = pool
        .candidates
        .into_iter()
        .map(|candidate| retrieval::Ranked {
            score: candidate.signals.score(),
            commit_time: candidate.commit_time,
            oid: candidate.oid.clone(),
            value: candidate,
        })
        .collect::<Vec<_>>();
    retrieval::sort(&mut ranked);

    let mut materials = ranked
        .into_iter()
        .take(limit)
        .map(|ranked| {
            let candidate = ranked.value;
            let mut basis = Vec::new();
            candidate.signals.describe(&mut basis);
            Material {
                subject: candidate.subject.clone(),
                paths: candidate.paths,
                confidence: lexical_confidence(&candidate.signals),
                basis,
                citations: vec![Citation::new(candidate.oid, candidate.subject)],
                detail: None,
            }
        })
        .collect::<Vec<_>>();
    retrieval::assign_citations(&mut materials);

    Ok(super::super::report(
        ReportKind::Search,
        materials,
        pool.matched_count,
        limit,
    ))
}
