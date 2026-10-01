use std::collections::{HashMap, HashSet};

use crate::{
    app::AppError,
    cache::{QuerySession, SearchFilter, SearchMaterial},
};

use super::super::{Citation, Confidence, Intent, Material, Report, ReportKind, retrieval};
use super::lexical_confidence;

const RRF_CONSTANT: usize = 60;
const MINIMUM_CANDIDATE_DEPTH: usize = 100;

struct FusedCandidate {
    commit_id: i64,
    oid: String,
    commit_time: i64,
    lexical: Option<retrieval::Scored>,
    lexical_rank: Option<usize>,
    semantic_rank: Option<usize>,
    semantic_material: Option<SearchMaterial>,
}

impl FusedCandidate {
    fn lexical(candidate: retrieval::Scored, rank: usize) -> Self {
        Self {
            commit_id: candidate.commit_id,
            oid: candidate.oid.clone(),
            commit_time: candidate.commit_time,
            lexical: Some(candidate),
            lexical_rank: Some(rank),
            semantic_rank: None,
            semantic_material: None,
        }
    }

    fn semantic(commit_id: i64, oid: String, commit_time: i64) -> Self {
        Self {
            commit_id,
            oid,
            commit_time,
            lexical: None,
            lexical_rank: None,
            semantic_rank: None,
            semantic_material: None,
        }
    }

    fn reciprocal_rank_score(&self) -> f64 {
        reciprocal_rank(self.lexical_rank) + reciprocal_rank(self.semantic_rank)
    }
}

fn reciprocal_rank(rank: Option<usize>) -> f64 {
    rank.map_or(0.0, |rank| 1.0 / (RRF_CONSTANT as f64 + rank as f64))
}

pub(crate) fn run(
    session: &QuerySession,
    intent: &Intent,
    limit: usize,
    scope: Option<&SearchFilter>,
    query_vector: &[f32],
) -> Result<Report, AppError> {
    let depth = limit.max(MINIMUM_CANDIDATE_DEPTH);
    let mut candidates = HashMap::<i64, FusedCandidate>::new();

    if let Some(pool) = retrieval::pool_with_depth(session, intent, depth, scope)? {
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

        let mut seen = HashSet::new();
        let lexical = ranked
            .into_iter()
            .filter_map(|candidate| {
                seen.insert(candidate.value.commit_id)
                    .then_some(candidate.value)
            })
            .take(depth)
            .collect::<Vec<_>>();
        for (index, candidate) in lexical.into_iter().enumerate() {
            let rank = index + 1;
            candidates.insert(
                candidate.commit_id,
                FusedCandidate::lexical(candidate, rank),
            );
        }
    }

    let semantic = session.semantic_top_k(query_vector, depth, scope)?;
    let mut seen = HashSet::new();
    for (index, candidate) in semantic
        .into_iter()
        .filter(|candidate| seen.insert(candidate.commit_id))
        .enumerate()
    {
        let rank = index + 1;
        let entry = candidates.entry(candidate.commit_id).or_insert_with(|| {
            FusedCandidate::semantic(
                candidate.commit_id,
                candidate.oid.clone(),
                candidate.commit_time,
            )
        });
        if entry.oid != candidate.oid || entry.commit_time != candidate.commit_time {
            return Err(AppError::operational(
                "error: lexical and semantic candidates disagree on commit identity",
            ));
        }
        entry.semantic_rank = Some(rank);
    }

    let semantic_only_ids = candidates
        .values()
        .filter(|candidate| candidate.lexical.is_none())
        .map(|candidate| candidate.commit_id)
        .collect::<Vec<_>>();
    for material in session.semantic_materials(&semantic_only_ids)? {
        let Some(candidate) = candidates.get_mut(&material.commit_id) else {
            return Err(AppError::operational(
                "error: semantic material has no matching ranked candidate",
            ));
        };
        if candidate.oid != material.oid || candidate.commit_time != material.commit_time {
            return Err(AppError::operational(
                "error: semantic material does not match its ranked commit identity",
            ));
        }
        candidate.semantic_material = Some(material);
    }
    if candidates
        .values()
        .any(|candidate| candidate.lexical.is_none() && candidate.semantic_material.is_none())
    {
        return Err(AppError::operational(
            "error: semantic result is missing its commit material",
        ));
    }

    let mut ranked = candidates.into_values().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .reciprocal_rank_score()
            .total_cmp(&left.reciprocal_rank_score())
            .then_with(|| left.oid.cmp(&right.oid))
    });
    let matched_count = ranked.len();
    let mut materials = ranked
        .into_iter()
        .take(limit)
        .map(materialize)
        .collect::<Vec<_>>();
    retrieval::assign_citations(&mut materials);

    let mut report = super::super::report(ReportKind::Search, materials, matched_count, limit);
    report.notices.push(format!(
        "Hybrid matched_count ({matched_count}) is the deduplicated union of the top {depth} lexical and semantic candidates (or fewer if a branch has fewer matches), not a total count of all relevant commits."
    ));
    Ok(report)
}

fn materialize(candidate: FusedCandidate) -> Material {
    let (subject, paths, confidence, basis) = match candidate.lexical {
        Some(lexical) => {
            let retrieval_basis = if candidate.semantic_rank.is_some() {
                "joint lexical + semantic retrieval"
            } else {
                "lexical retrieval"
            };
            let mut basis = vec![retrieval_basis.to_owned()];
            lexical.signals.describe(&mut basis);
            (
                lexical.subject.clone(),
                lexical.paths,
                lexical_confidence(&lexical.signals),
                basis,
            )
        }
        None => {
            let semantic = candidate
                .semantic_material
                .expect("semantic-only candidates have fetched materials");
            (
                semantic.subject,
                semantic.paths,
                Confidence::Low,
                vec!["semantic retrieval".to_owned()],
            )
        }
    };
    let citation_subject = subject.clone();
    Material {
        subject,
        paths,
        confidence,
        basis,
        citations: vec![Citation::new(candidate.oid, citation_subject)],
        detail: None,
        patch: None,
    }
}

#[cfg(test)]
mod tests {
    use super::reciprocal_rank;

    #[test]
    fn reciprocal_rank_uses_one_based_ranks_and_constant_sixty() {
        assert_eq!(reciprocal_rank(Some(1)), 1.0 / 61.0);
        assert_eq!(reciprocal_rank(Some(2)), 1.0 / 62.0);
        assert_eq!(reciprocal_rank(None), 0.0);
    }
}
