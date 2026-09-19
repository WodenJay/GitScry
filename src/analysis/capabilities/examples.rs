use rusqlite::Connection;

use crate::app::AppError;

use super::super::retrieval;
use super::super::{Citation, Confidence, Detail, Intent, Material, Report, ReportKind};
use super::{confidence, empty};

/// How much one anchored path overlap raises a candidate.
const ANCHOR_WEIGHT: f64 = 4.0;
/// A change that moved several paths together, like its neighbours, is more reusable.
const COHERENT_WEIGHT: f64 = 2.0;
/// Reverted work is still history, but it is not a precedent to follow.
const REVERT_DEMOTION: f64 = 8.0;

pub(crate) fn run(
    connection: &Connection,
    intent: &Intent,
    limit: usize,
) -> Result<Report, AppError> {
    let Some(pool) = retrieval::pool(connection, intent, limit)? else {
        return Ok(empty(ReportKind::Examples));
    };
    let reverts = retrieval::reverts(connection)?;
    let steps = pool.steps(connection)?;

    let mut ranked = pool
        .candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            let anchored = candidate.signals.matched_anchors();
            let coherent = coherent_change(&pool, index);
            let mut score = candidate.signals.score() + anchored as f64 * ANCHOR_WEIGHT;
            score += if coherent { COHERENT_WEIGHT } else { 0.0 };
            let reverted = reverts.reverting(&candidate.oid).is_some();
            score -= if reverted { REVERT_DEMOTION } else { 0.0 };
            retrieval::Ranked {
                score,
                commit_time: candidate.commit_time,
                oid: candidate.oid.clone(),
                value: (index, anchored, coherent),
            }
        })
        .collect::<Vec<_>>();
    retrieval::sort(&mut ranked);

    let mut materials = Vec::new();
    for ranked in ranked.into_iter().take(limit) {
        let (index, anchored, coherent) = ranked.value;
        let candidate = &pool.candidates[index];
        let mut basis = Vec::new();
        candidate.signals.describe(&mut basis);
        if anchored > 0 {
            basis.push(format!("anchored path match ({anchored})"));
        }
        if coherent {
            basis.push("coherent multi-path change".to_owned());
        }
        let mut citations = vec![Citation::new(
            candidate.oid.clone(),
            candidate.subject.clone(),
        )];
        let mut confidence = confidence(&candidate.signals);
        if let Some(revert) = reverts.reverting(&candidate.oid) {
            citations.push(
                Citation::new(revert.oid.clone(), revert.subject.clone()).noting("later reverted"),
            );
            basis.push("demoted: later reverted".to_owned());
            confidence = Confidence::Low;
        }
        materials.push(Material {
            subject: candidate.subject.clone(),
            paths: candidate.paths.clone(),
            confidence,
            basis,
            citations,
            detail: (!steps[index].is_empty()).then_some(Detail::Steps(steps[index].clone())),
        });
    }
    retrieval::assign_citations(&mut materials);

    Ok(super::super::report(
        ReportKind::Examples,
        materials,
        pool.matched_count,
        limit,
    ))
}

/// Whether this change moved several paths together, the way another candidate did.
fn coherent_change(pool: &retrieval::Pool, index: usize) -> bool {
    let Some(candidate) = pool.candidates.get(index) else {
        return false;
    };
    candidate.paths.len() >= 2
        && (0..pool.candidates.len())
            .filter(|other| *other != index)
            .any(|other| pool.shared_paths(index, other) >= 2)
}
