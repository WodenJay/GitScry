use std::collections::{HashMap, HashSet};

use crate::{
    app::AppError,
    cache::{QuerySession, SearchFilter, SearchMaterial, SemanticCandidate},
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

fn rank_candidates(
    lexical: Vec<retrieval::Scored>,
    mut semantic: Vec<SemanticCandidate>,
    depth: usize,
) -> Result<HashMap<i64, FusedCandidate>, AppError> {
    let mut lexical = lexical
        .into_iter()
        .map(|candidate| retrieval::Ranked {
            score: candidate.signals.score(),
            commit_time: candidate.commit_time,
            oid: candidate.oid.clone(),
            value: candidate,
        })
        .collect::<Vec<_>>();
    retrieval::sort(&mut lexical);

    let mut candidates = HashMap::new();
    let mut seen = HashSet::new();
    for (index, candidate) in lexical
        .into_iter()
        .map(|candidate| candidate.value)
        .filter(|candidate| seen.insert(candidate.commit_id))
        .take(depth)
        .enumerate()
    {
        candidates.insert(
            candidate.commit_id,
            FusedCandidate::lexical(candidate, index + 1),
        );
    }

    semantic.sort();
    let mut seen = HashSet::new();
    for (index, candidate) in semantic
        .into_iter()
        .filter(|candidate| seen.insert(candidate.commit_id))
        .take(depth)
        .enumerate()
    {
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
        entry.semantic_rank = Some(index + 1);
    }
    Ok(candidates)
}

fn sort_candidates(candidates: impl IntoIterator<Item = FusedCandidate>) -> Vec<FusedCandidate> {
    let mut candidates = candidates.into_iter().collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .reciprocal_rank_score()
            .total_cmp(&left.reciprocal_rank_score())
            .then_with(|| left.oid.cmp(&right.oid))
    });
    candidates
}

pub(crate) fn run(
    session: &QuerySession,
    intent: &Intent,
    limit: usize,
    scope: Option<&SearchFilter>,
    query_vectors: &[Vec<f32>],
) -> Result<Report, AppError> {
    let depth = limit.max(MINIMUM_CANDIDATE_DEPTH);
    let lexical = retrieval::pool_with_depth(session, intent, depth, scope)?
        .map_or_else(Vec::new, |pool| pool.candidates);
    let semantic = session.semantic_top_k(query_vectors, depth, scope)?;
    let mut candidates = rank_candidates(lexical, semantic, depth)?;
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

    let ranked = sort_candidates(candidates.into_values());
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
    use super::{
        FusedCandidate, SearchMaterial, SemanticCandidate, lexical_confidence, materialize,
        rank_candidates, reciprocal_rank, sort_candidates,
    };
    use crate::analysis::retrieval;

    fn lexical_candidate(commit_id: i64, oid: &str) -> retrieval::Scored {
        let mut candidate = retrieval::test_scored_candidate(commit_id, oid, "test");
        candidate.commit_time = 0;
        candidate
    }

    fn semantic_candidate(commit_id: i64, oid: &str, cosine: f64) -> SemanticCandidate {
        SemanticCandidate {
            commit_id,
            oid: oid.to_owned(),
            commit_time: 0,
            cosine,
        }
    }

    fn rank(
        lexical: Vec<retrieval::Scored>,
        semantic: Vec<SemanticCandidate>,
        depth: usize,
    ) -> Vec<FusedCandidate> {
        sort_candidates(
            rank_candidates(lexical, semantic, depth)
                .unwrap()
                .into_values(),
        )
    }

    #[test]
    fn reciprocal_rank_uses_one_based_ranks_and_constant_sixty() {
        assert_eq!(reciprocal_rank(Some(1)), 1.0 / 61.0);
        assert_eq!(reciprocal_rank(Some(2)), 1.0 / 62.0);
        assert_eq!(reciprocal_rank(None), 0.0);
    }

    #[test]
    fn rank_candidates_deduplicates_overlapping_branches_before_fusion() {
        let ranked = rank(
            vec![lexical_candidate(1, "b"), lexical_candidate(3, "c")],
            vec![
                semantic_candidate(1, "b", 0.9),
                semantic_candidate(2, "a", 0.8),
                semantic_candidate(1, "b", 0.7),
            ],
            100,
        );

        assert_eq!(ranked.len(), 3);
        assert_eq!(
            ranked
                .iter()
                .map(|candidate| candidate.oid.as_str())
                .collect::<Vec<_>>(),
            ["b", "a", "c"]
        );
        assert_eq!(ranked[0].lexical_rank, Some(1));
        assert_eq!(ranked[0].semantic_rank, Some(1));
        assert!((ranked[0].reciprocal_rank_score() - 2.0 / 61.0).abs() < f64::EPSILON);
        assert_eq!(ranked[1].lexical_rank, None);
        assert_eq!(ranked[1].semantic_rank, Some(2));
        assert_eq!(ranked[2].lexical_rank, Some(2));
        assert_eq!(ranked[2].semantic_rank, None);
    }

    #[test]
    fn rank_candidates_returns_semantic_only_results_with_empty_lexical_recall() {
        let ranked = rank(
            Vec::new(),
            vec![semantic_candidate(1, "semantic-only", 0.9)],
            100,
        );

        assert_eq!(ranked.len(), 1);
        assert!(ranked[0].lexical.is_none());
        assert_eq!(ranked[0].semantic_rank, Some(1));
    }

    #[test]
    fn rank_candidates_handles_empty_and_lexical_only_recall() {
        assert!(rank(Vec::new(), Vec::new(), 100).is_empty());
        let ranked = rank(vec![lexical_candidate(1, "lexical-only")], Vec::new(), 100);
        assert_eq!(ranked.len(), 1);
        assert!(ranked[0].lexical.is_some());
        assert_eq!(ranked[0].semantic_rank, None);
    }

    #[test]
    fn equal_fused_scores_break_ties_by_oid() {
        let ranked = rank(
            vec![lexical_candidate(1, "b")],
            vec![semantic_candidate(2, "a", 0.9)],
            100,
        );

        assert_eq!(ranked[0].oid, "a");
        assert_eq!(ranked[1].oid, "b");
        assert_eq!(
            ranked[0].reciprocal_rank_score(),
            ranked[1].reciprocal_rank_score()
        );
    }

    #[test]
    fn rank_candidates_caps_each_branch_at_depth_above_one_hundred() {
        let lexical = (0..105)
            .map(|id| lexical_candidate(id, &format!("{id:040x}")))
            .collect();
        let semantic = (0..105)
            .map(|id| semantic_candidate(id, &format!("{id:040x}"), 0.9))
            .collect();

        let ranked = rank(lexical, semantic, 101);

        assert_eq!(ranked.len(), 101);
        assert_eq!(ranked.first().unwrap().oid, format!("{:040x}", 0));
        assert_eq!(ranked.last().unwrap().oid, format!("{:040x}", 100));
    }

    #[test]
    fn semantic_only_material_has_low_confidence_and_citation() {
        let mut candidate = rank(
            Vec::new(),
            vec![semantic_candidate(1, "semantic-only", 0.9)],
            100,
        )
        .pop()
        .unwrap();
        candidate.semantic_material = Some(SearchMaterial {
            commit_id: 1,
            oid: "semantic-only".to_owned(),
            commit_time: 0,
            subject: "Semantic result".to_owned(),
            paths: vec![b"src/semantic.rs".to_vec()],
        });

        let material = materialize(candidate);

        assert!(matches!(
            material.confidence,
            crate::analysis::Confidence::Low
        ));
        assert_eq!(material.basis, ["semantic retrieval"]);
        assert_eq!(material.paths, [b"src/semantic.rs".to_vec()]);
        assert_eq!(material.citations.len(), 1);
    }

    #[test]
    fn joint_material_preserves_lexical_confidence() {
        let lexical = lexical_candidate(1, "joint");
        let expected_confidence = lexical_confidence(&lexical.signals);
        let mut candidate = FusedCandidate::lexical(lexical, 1);
        candidate.semantic_rank = Some(1);

        let material = materialize(candidate);

        assert!(matches!(
            expected_confidence,
            crate::analysis::Confidence::Medium
        ));
        assert!(matches!(
            material.confidence,
            crate::analysis::Confidence::Medium
        ));
        assert_eq!(material.basis[0], "joint lexical + semantic retrieval");
    }
}
