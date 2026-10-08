use crate::{
    analysis::patch_relationship::{self, CompletePatch},
    app::AppError,
    cache::{PatchFingerprints, QuerySession, SearchFilter},
    git::Repository,
};

use super::super::retrieval;
use super::super::{
    Citation, Confidence, Detail, Intent, Material, PatchEquivalence, PatchGroupMember,
    PatchGrouping, PatchIndeterminate, Report, ReportKind,
};
use super::lexical_confidence;
use std::collections::{HashMap, HashSet};

/// How much one anchored path overlap raises a candidate.
const ANCHOR_WEIGHT: f64 = 4.0;
/// A change that moved several paths together, like its neighbours, is more reusable.
const COHERENT_WEIGHT: f64 = 2.0;
/// Above this many paths a change is a batch, not a reusable multi-file pattern.
const COHERENT_PATH_LIMIT: usize = 12;
/// Reverted work is still history, but it is not a precedent to follow.
const REVERT_DEMOTION: f64 = 8.0;

#[derive(Clone, Copy)]
struct CandidateRank {
    order: usize,
    index: usize,
    anchored: usize,
    coherent: bool,
}

struct PatchGroup {
    patch: CompletePatch,
    fingerprint: Vec<u8>,
    representative: CandidateRank,
    members: Vec<PatchGroupMember>,
}

pub(crate) fn run(
    session: &QuerySession,
    intent: &Intent,
    limit: usize,
    scope: Option<&SearchFilter>,
) -> Result<Report, AppError> {
    // Grouping needs every lexical match; applying the ordinary candidate depth first could
    // hide a distinct precedent behind many equivalent commits.
    let Some(pool) = retrieval::pool(session, intent, usize::MAX, scope)? else {
        return Ok(super::super::empty_report(ReportKind::Examples));
    };
    let reverts = retrieval::reverts(session, scope)?;
    let mut ranked = Vec::new();
    for (index, candidate) in pool.candidates.iter().enumerate() {
        let anchored = candidate.signals.matched_anchors();
        let coherent = coherent_change(&pool, index);
        let mut score = candidate.signals.identified_score() + anchored as f64 * ANCHOR_WEIGHT;
        score += if coherent { COHERENT_WEIGHT } else { 0.0 };
        // Work that was later undone, and the revert itself, are not precedents to follow.
        if reverts.is_revert(&candidate.oid) || retrieval::link(&reverts, candidate).is_some() {
            score -= REVERT_DEMOTION;
        }
        ranked.push(retrieval::Ranked {
            score,
            commit_time: candidate.commit_time,
            oid: candidate.oid.clone(),
            value: (index, anchored, coherent),
        });
    }
    retrieval::sort(&mut ranked);

    let repository = Repository::discover()?;
    let fingerprints = PatchFingerprints::open(
        &repository.common_dir,
        patch_relationship::NORMALIZATION_VERSION,
    )?;
    let mut groups = Vec::<PatchGroup>::new();
    let mut ungrouped = Vec::new();
    let mut empty_count = 0;
    let mut candidate_oids = HashSet::new();
    let mut indeterminate = Vec::new();

    // Preserve the existing ranking unchanged: a group's representative is its highest-ranked
    // lexical candidate, and group size never contributes to its score.
    for (order, ranked_candidate) in ranked.into_iter().enumerate() {
        let (index, anchored, coherent) = ranked_candidate.value;
        let candidate_rank = CandidateRank {
            order,
            index,
            anchored,
            coherent,
        };
        let candidate = &pool.candidates[index];
        candidate_oids.insert(candidate.oid.clone());
        match inspect_and_cache(&repository, &fingerprints, &candidate.oid)? {
            Ok(patch) if patch.is_empty() => empty_count += 1,
            Ok(patch) => {
                let fingerprint = patch.fingerprint();
                let member = PatchGroupMember {
                    oid: candidate.oid.clone(),
                    subject: candidate.subject.clone(),
                };
                if let Some(group) = groups.iter_mut().find(|group| {
                    group.fingerprint == fingerprint && group.patch.equivalent(&patch)
                }) {
                    group.members.push(member);
                } else {
                    groups.push(PatchGroup {
                        patch,
                        fingerprint,
                        representative: candidate_rank,
                        members: vec![member],
                    });
                }
            }
            Err(reason) => {
                indeterminate.push(PatchIndeterminate {
                    commit_oid: candidate.oid.clone(),
                    reason,
                });
                ungrouped.push(candidate_rank);
            }
        }
    }

    let eligible = match scope {
        Some(scope) => session.commits_scoped(scope)?,
        None => session.commits()?,
    };
    let eligible_count = eligible.len();
    let mut cached = HashMap::new();
    let mut object_ids = HashSet::new();
    for commit in &eligible {
        if candidate_oids.contains(&commit.oid) {
            continue;
        }
        if let Some(record) = fingerprints.get(&commit.oid)? {
            if matches!(record.integrity.as_str(), "complete" | "empty") {
                object_ids.extend(record.objects.lines().map(str::to_owned));
            }
            cached.insert(commit.oid.clone(), record);
        }
    }
    let mut object_ids: Vec<_> = object_ids.into_iter().collect();
    object_ids.sort();
    let missing_objects: HashSet<_> = repository
        .missing_objects(&object_ids)?
        .into_iter()
        .collect();

    let mut groups_by_fingerprint = HashMap::<Vec<u8>, Vec<usize>>::new();
    for (index, group) in groups.iter().enumerate() {
        groups_by_fingerprint
            .entry(group.fingerprint.clone())
            .or_default()
            .push(index);
    }

    let mut checked_count = 0;
    for commit in &eligible {
        checked_count += 1;
        // These candidates were already inspected and either added to a group or retained
        // independently when empty or indeterminate.
        if candidate_oids.contains(&commit.oid) {
            continue;
        }

        let mut cached_hash = None;
        if let Some(record) = cached.remove(&commit.oid)
            && matches!(record.integrity.as_str(), "complete" | "empty")
        {
            if record
                .objects
                .lines()
                .any(|object| missing_objects.contains(object))
            {
                let reason = "cached patch source objects are unavailable".to_owned();
                fingerprints.put(&commit.oid, "indeterminate", None, None, &[])?;
                indeterminate.push(PatchIndeterminate {
                    commit_oid: commit.oid.clone(),
                    reason,
                });
                continue;
            }
            if record.integrity == "empty" && record.forward_fingerprint.is_some() {
                continue;
            }
            cached_hash = record.forward_fingerprint;
        }

        let (fingerprint, inspected) = match cached_hash {
            Some(fingerprint) => (fingerprint, None),
            None => match inspect_and_cache(&repository, &fingerprints, &commit.oid)? {
                Ok(patch) if patch.is_empty() => continue,
                Ok(patch) => (patch.fingerprint(), Some(patch)),
                Err(reason) => {
                    indeterminate.push(PatchIndeterminate {
                        commit_oid: commit.oid.clone(),
                        reason,
                    });
                    continue;
                }
            },
        };
        let Some(group_indices) = groups_by_fingerprint.get(&fingerprint) else {
            continue;
        };
        // Fingerprints only narrow candidates; full normalized patches certify membership.
        let patch = match inspected {
            Some(patch) => patch,
            None => match inspect_and_cache(&repository, &fingerprints, &commit.oid)? {
                Ok(patch) => patch,
                Err(reason) => {
                    indeterminate.push(PatchIndeterminate {
                        commit_oid: commit.oid.clone(),
                        reason,
                    });
                    continue;
                }
            },
        };
        let matching_group = group_indices
            .iter()
            .copied()
            .find(|index| groups[*index].patch.equivalent(&patch));
        if let Some(index) = matching_group {
            let (subject, _) = retrieval::message_parts(&commit.message);
            groups[index].members.push(PatchGroupMember {
                oid: commit.oid.clone(),
                subject,
            });
        }
    }

    let complete = indeterminate.is_empty() && checked_count == eligible_count;
    let group_count = groups.len();
    let ungrouped_count = ungrouped.len();
    let result_count = group_count + ungrouped_count;
    let mut ranked_results = Vec::with_capacity(result_count);
    for group in groups {
        let representative = group.representative;
        let representative_oid = pool.candidates[representative.index].oid.clone();
        let member_count = group.members.len();
        ranked_results.push((
            representative.order,
            representative,
            Some(PatchEquivalence {
                representative_oid,
                member_count,
                members: group.members,
                complete,
            }),
        ));
    }
    ranked_results.extend(
        ungrouped
            .into_iter()
            .map(|candidate| (candidate.order, candidate, None)),
    );
    ranked_results.sort_by_key(|(order, _, _)| *order);

    let mut materials = Vec::new();
    for (_, candidate, patch_equivalence) in ranked_results.into_iter().take(limit) {
        materials.push(materialize(
            session,
            &pool,
            &reverts,
            candidate,
            patch_equivalence,
        )?);
    }
    retrieval::assign_citations(&mut materials);

    let mut report =
        super::super::report(ReportKind::Examples, materials, pool.matched_count, limit);
    report.truncated = result_count > limit;
    report.patch_grouping = Some(PatchGrouping {
        candidate_count: pool.matched_count,
        empty_count,
        group_count,
        ungrouped_count,
        eligible_count,
        checked_count,
        complete,
        indeterminate,
    });
    Ok(report)
}

fn inspect_and_cache(
    repository: &Repository,
    fingerprints: &PatchFingerprints,
    oid: &str,
) -> Result<Result<CompletePatch, String>, AppError> {
    match patch_relationship::inspect(repository, oid) {
        Ok(patch) => {
            let fingerprint = patch.fingerprint();
            let inverse_fingerprint = patch.inverse_fingerprint();
            fingerprints.put(
                oid,
                if patch.is_empty() {
                    "empty"
                } else {
                    "complete"
                },
                Some(&fingerprint),
                Some(&inverse_fingerprint),
                &patch.objects,
            )?;
            Ok(Ok(patch))
        }
        Err(reason) => {
            fingerprints.put(oid, "indeterminate", None, None, &[])?;
            Ok(Err(reason))
        }
    }
}

fn materialize(
    session: &QuerySession,
    pool: &retrieval::Pool,
    reverts: &retrieval::RevertIndex,
    ranked: CandidateRank,
    patch_equivalence: Option<PatchEquivalence>,
) -> Result<Material, AppError> {
    let candidate = &pool.candidates[ranked.index];
    let mut basis = Vec::new();
    candidate.signals.describe_identified(&mut basis);
    if ranked.anchored > 0 {
        basis.push(format!("anchored path match ({})", ranked.anchored));
    }
    if ranked.coherent {
        basis.push("coherent multi-path change".to_owned());
    }
    let mut citations = vec![Citation::new(
        candidate.oid.clone(),
        candidate.subject.clone(),
    )];
    let mut confidence = lexical_confidence(&candidate.signals);
    if reverts.is_revert(&candidate.oid) {
        basis.push("demoted: a revert, not a precedent".to_owned());
        confidence = Confidence::Low;
    } else if let Some(revert) = retrieval::link(reverts, candidate) {
        citations.push(
            Citation::new(revert.oid.clone(), revert.subject.clone()).noting("later reverted"),
        );
        basis.push("demoted: later reverted".to_owned());
        confidence = Confidence::Low;
    }
    // Steps are only read for the results actually returned, not the whole pool.
    let steps = retrieval::steps(session, &candidate.oid)?;
    let detail = if steps.is_empty() && patch_equivalence.is_none() {
        None
    } else {
        Some(Detail::Steps {
            steps,
            patch_equivalence,
        })
    };
    Ok(Material {
        subject: candidate.subject.clone(),
        paths: candidate.paths.clone(),
        confidence,
        basis,
        citations,
        detail,
        patch: None,
    })
}

/// Whether this change moved several paths together, the way another candidate did.
fn coherent_change(pool: &retrieval::Pool, index: usize) -> bool {
    let Some(candidate) = pool.candidates.get(index) else {
        return false;
    };
    candidate.paths.len() >= 2
        && candidate.paths.len() <= COHERENT_PATH_LIMIT
        && (0..pool.candidates.len())
            .filter(|other| *other != index)
            .any(|other| pool.shared_paths(index, other) >= 2)
}
