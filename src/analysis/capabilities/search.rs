use crate::{
    app::AppError,
    cache::{QuerySession, SearchFilter},
};

use super::super::retrieval;
use super::super::{Citation, Intent, Material, Report, ReportKind};
use super::lexical_confidence;

pub(crate) fn run(
    session: &QuerySession,
    intent: &Intent,
    limit: usize,
) -> Result<Report, AppError> {
    run_with_scope(session, intent, limit, None)
}

pub(crate) fn run_scoped(
    session: &QuerySession,
    intent: &Intent,
    limit: usize,
    scope: &SearchFilter,
) -> Result<Report, AppError> {
    run_with_scope(session, intent, limit, Some(scope))
}

fn run_with_scope(
    session: &QuerySession,
    intent: &Intent,
    limit: usize,
    scope: Option<&SearchFilter>,
) -> Result<Report, AppError> {
    let pool = retrieval::pool(session, intent, limit, scope)?;
    let Some(pool) = pool else {
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
                patch: None,
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
