//! Complete inverse-patch leads related to the failure query, separate from failures.
use std::collections::{HashMap, HashSet};

use crate::{
    analysis::{
        capabilities::failures::MIN_SHARED_TERMS,
        material::{
            ExplicitRevert, FailureLeads, InverseLead, PatchEquivalence, PatchGroupMember,
            PatchIndeterminate, PatchMaterial,
        },
        patch_relationship::{self, CompletePatch},
        retrieval::{self, RevertIndex},
    },
    app::AppError,
    cache::{PatchFingerprints, QuerySession, SearchFilter},
    git::Repository,
};

pub(super) struct Scan {
    pub(super) report: FailureLeads,
    pub(super) equivalences: HashMap<String, PatchEquivalence>,
}

struct SourceGroup {
    patch: Option<CompletePatch>,
    inverse_fingerprint: Vec<u8>,
    representative_oid: String,
    representative_subject: String,
    candidate_order: usize,
    members: Vec<PatchGroupMember>,
    member_oids: HashSet<String>,
}

struct InverseGroup {
    patch: Option<CompletePatch>,
    fingerprint: Vec<u8>,
    representative_oid: String,
    representative_message: Vec<u8>,
    first_history_order: usize,
    members: Vec<PatchGroupMember>,
    member_oids: HashSet<String>,
    matching_sources: Vec<usize>,
}

struct LeadRef {
    source: usize,
    inverse: usize,
}

pub(super) fn run(
    session: &QuerySession,
    intent: &crate::analysis::Intent,
    limit: usize,
    scope: Option<&SearchFilter>,
    reverts: &RevertIndex,
) -> Result<Scan, AppError> {
    let Some(pool) = retrieval::pool(session, intent, usize::MAX, scope)? else {
        return Ok(Scan {
            report: empty(limit),
            equivalences: HashMap::new(),
        });
    };
    let repository = Repository::discover()?;
    let fingerprints = PatchFingerprints::open(
        &repository.common_dir,
        patch_relationship::NORMALIZATION_VERSION,
    )?;
    let mut indeterminate = Vec::new();
    let mut uninspectable_sources = HashSet::new();
    let mut sources = Vec::<SourceGroup>::new();
    let mut sources_by_fingerprint = HashMap::<Vec<u8>, Vec<usize>>::new();

    let mut ranked = pool
        .candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| candidate.signals.shared_terms() >= MIN_SHARED_TERMS)
        .map(|(index, candidate)| retrieval::Ranked {
            score: candidate.signals.identified_score(),
            commit_time: candidate.commit_time,
            oid: candidate.oid.clone(),
            value: index,
        })
        .collect::<Vec<_>>();
    retrieval::sort(&mut ranked);

    for (candidate_order, ranked_candidate) in ranked.into_iter().enumerate() {
        let candidate = &pool.candidates[ranked_candidate.value];
        let patch = match patch_relationship::inspect_and_cache(
            &repository,
            &fingerprints,
            &candidate.oid,
        )? {
            Ok(patch) => patch,
            Err(reason) => {
                uninspectable_sources.insert(candidate.oid.clone());
                indeterminate.push(PatchIndeterminate {
                    commit_oid: candidate.oid.clone(),
                    reason,
                });
                continue;
            }
        };
        if patch.is_empty() {
            continue;
        }

        let fingerprint = patch.fingerprint();
        let matching_group = sources_by_fingerprint
            .get(&fingerprint)
            .into_iter()
            .flatten()
            .copied()
            .find(|index| {
                sources[*index]
                    .patch
                    .as_ref()
                    .is_some_and(|group| group.equivalent(&patch))
            });
        let member = PatchGroupMember {
            oid: candidate.oid.clone(),
            subject: candidate.subject.clone(),
        };
        if let Some(index) = matching_group {
            let group = &mut sources[index];
            add_member(&mut group.members, &mut group.member_oids, member);
            continue;
        }

        let inverse_fingerprint = patch.inverse_fingerprint();
        let index = sources.len();
        sources_by_fingerprint
            .entry(fingerprint.clone())
            .or_default()
            .push(index);
        sources.push(SourceGroup {
            patch: Some(patch),
            inverse_fingerprint,
            representative_oid: candidate.oid.clone(),
            representative_subject: candidate.subject.clone(),
            candidate_order,
            members: vec![member],
            member_oids: HashSet::from([candidate.oid.clone()]),
        });
    }

    let eligible = match scope {
        Some(scope) => session.commits_scoped(scope)?,
        None => session.commits()?,
    };
    let eligible_count = eligible.len();
    let mut cached = HashMap::new();
    let mut object_ids = HashSet::new();
    for commit in &eligible {
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

    let mut sources_by_inverse_fingerprint = HashMap::<Vec<u8>, Vec<usize>>::new();
    for (index, source) in sources.iter().enumerate() {
        sources_by_inverse_fingerprint
            .entry(source.inverse_fingerprint.clone())
            .or_default()
            .push(index);
    }

    let mut inverse_groups = Vec::<InverseGroup>::new();
    let mut inverse_groups_by_fingerprint = HashMap::<Vec<u8>, Vec<usize>>::new();
    let mut checked_count = 0;
    for (history_order, commit) in eligible.iter().enumerate() {
        checked_count += 1;
        if uninspectable_sources.contains(&commit.oid) {
            continue;
        }

        let stored = cached.remove(&commit.oid);
        let cached_empty = stored
            .as_ref()
            .is_some_and(|record| record.integrity == "empty");
        if stored.as_ref().is_some_and(|record| {
            matches!(record.integrity.as_str(), "complete" | "empty")
                && record
                    .objects
                    .lines()
                    .any(|object| missing_objects.contains(object))
        }) {
            fingerprints.put(&commit.oid, "indeterminate", None, None, &[])?;
            indeterminate.push(PatchIndeterminate {
                commit_oid: commit.oid.clone(),
                reason: "cached patch source objects are unavailable".to_owned(),
            });
            continue;
        }

        let cached_hashes = stored
            .filter(|record| matches!(record.integrity.as_str(), "complete" | "empty"))
            .and_then(|record| Some((record.forward_fingerprint?, record.inverse_fingerprint?)));
        let mut inspected = None;
        let (forward, inverse, empty) = match cached_hashes {
            Some((forward, inverse)) => (forward, inverse, cached_empty),
            None => match patch_relationship::inspect_and_cache(
                &repository,
                &fingerprints,
                &commit.oid,
            )? {
                Ok(patch) => {
                    let forward = patch.fingerprint();
                    let inverse = patch.inverse_fingerprint();
                    let empty = patch.is_empty();
                    inspected = Some(patch);
                    (forward, inverse, empty)
                }
                Err(reason) => {
                    indeterminate.push(PatchIndeterminate {
                        commit_oid: commit.oid.clone(),
                        reason,
                    });
                    continue;
                }
            },
        };
        if empty {
            continue;
        }

        let (equivalent_sources, inverse_sources) = matching_sources(
            &forward,
            &inverse,
            &sources_by_fingerprint,
            &sources_by_inverse_fingerprint,
        );
        if equivalent_sources.is_empty() && inverse_sources.is_empty() {
            continue;
        }
        let patch = match inspected {
            Some(patch) => patch,
            None => match patch_relationship::inspect(&repository, &commit.oid) {
                Ok(patch) => patch,
                Err(reason) => {
                    fingerprints.put(&commit.oid, "indeterminate", None, None, &[])?;
                    indeterminate.push(PatchIndeterminate {
                        commit_oid: commit.oid.clone(),
                        reason,
                    });
                    continue;
                }
            },
        };
        let member = PatchGroupMember {
            oid: commit.oid.clone(),
            subject: retrieval::message_parts(&commit.message).0,
        };
        for source_index in equivalent_sources {
            let source = &mut sources[source_index];
            if source
                .patch
                .as_ref()
                .is_some_and(|source_patch| source_patch.equivalent(&patch))
            {
                add_member(
                    &mut source.members,
                    &mut source.member_oids,
                    member_for(&member),
                );
            }
        }

        let matching_sources = inverse_sources
            .into_iter()
            .filter(|source_index| {
                !sources[*source_index].member_oids.contains(&commit.oid)
                    && sources[*source_index]
                        .patch
                        .as_ref()
                        .is_some_and(|source_patch| source_patch.inverse_equivalent(&patch))
            })
            .collect::<Vec<_>>();
        if matching_sources.is_empty() {
            continue;
        }

        let fingerprint = patch.fingerprint();
        let existing_group = inverse_groups_by_fingerprint
            .get(&fingerprint)
            .into_iter()
            .flatten()
            .copied()
            .find(|index| {
                inverse_groups[*index]
                    .patch
                    .as_ref()
                    .is_some_and(|group| group.equivalent(&patch))
            });
        if let Some(index) = existing_group {
            let group = &mut inverse_groups[index];
            add_member(&mut group.members, &mut group.member_oids, member);
            for source_index in matching_sources {
                if !group.matching_sources.contains(&source_index) {
                    group.matching_sources.push(source_index);
                }
            }
        } else {
            let index = inverse_groups.len();
            inverse_groups_by_fingerprint
                .entry(fingerprint.clone())
                .or_default()
                .push(index);
            inverse_groups.push(InverseGroup {
                patch: Some(patch),
                fingerprint,
                representative_oid: commit.oid.clone(),
                representative_message: commit.message.clone(),
                first_history_order: history_order,
                members: vec![member],
                member_oids: HashSet::from([commit.oid.clone()]),
                matching_sources,
            });
        }
    }

    let complete = indeterminate.is_empty() && checked_count == eligible_count;
    let mut equivalences = HashMap::new();
    for source in &sources {
        if source.members.len() < 2 {
            continue;
        }
        for member in &source.members {
            equivalences.insert(
                member.oid.clone(),
                PatchEquivalence {
                    representative_oid: member.oid.clone(),
                    member_count: source.members.len(),
                    members: clone_members(&source.members),
                    complete,
                },
            );
        }
    }

    let mut refs = Vec::new();
    for (inverse_index, inverse) in inverse_groups.iter().enumerate() {
        let query_group = sources_by_fingerprint
            .get(&inverse.fingerprint)
            .into_iter()
            .flatten()
            .copied()
            .find(|source_index| {
                sources[*source_index]
                    .patch
                    .as_ref()
                    .zip(inverse.patch.as_ref())
                    .is_some_and(|(source_patch, inverse_patch)| {
                        source_patch.equivalent(inverse_patch)
                    })
            });
        for source_index in &inverse.matching_sources {
            if Some(*source_index) == query_group
                || query_group.is_some_and(|query_index| {
                    sources[query_index].candidate_order < sources[*source_index].candidate_order
                })
            {
                continue;
            }
            refs.push(LeadRef {
                source: *source_index,
                inverse: inverse_index,
            });
        }
    }
    refs.sort_by_key(|lead| {
        (
            sources[lead.source].candidate_order,
            inverse_groups[lead.inverse].first_history_order,
        )
    });
    let matched_count = refs.len();
    let returned_count = matched_count.min(limit);

    let mut source_materials = HashMap::<usize, PatchMaterial>::new();
    let mut inverse_materials = HashMap::<usize, PatchMaterial>::new();
    let mut leads = Vec::with_capacity(returned_count);
    for lead in refs.into_iter().take(limit) {
        if let std::collections::hash_map::Entry::Vacant(entry) =
            source_materials.entry(lead.source)
        {
            let source = &mut sources[lead.source];
            let patch = source.patch.take().expect("source patch remains complete");
            entry.insert(PatchMaterial::from_complete(
                source.representative_oid.clone(),
                source.representative_subject.as_bytes(),
                patch,
                None,
            ));
        }
        if let std::collections::hash_map::Entry::Vacant(entry) =
            inverse_materials.entry(lead.inverse)
        {
            let inverse = &mut inverse_groups[lead.inverse];
            let patch = inverse
                .patch
                .take()
                .expect("inverse patch remains complete");
            entry.insert(PatchMaterial::from_complete(
                inverse.representative_oid.clone(),
                &inverse.representative_message,
                patch,
                Some("inverse"),
            ));
        }

        let source = &sources[lead.source];
        let inverse = &inverse_groups[lead.inverse];
        let representative_source = source
            .members
            .first()
            .expect("query patch group has a member");
        let representative_inverse = inverse
            .members
            .first()
            .expect("inverse patch group has a member");
        let explicit_reverts = source
            .members
            .iter()
            .chain(&inverse.members)
            .filter_map(|member| {
                let target_oid = reverts.target_of(&member.oid)?;
                let connects_lead = (source.member_oids.contains(&member.oid)
                    && inverse.member_oids.contains(target_oid))
                    || (inverse.member_oids.contains(&member.oid)
                        && source.member_oids.contains(target_oid));
                connects_lead.then(|| ExplicitRevert {
                    revert_oid: member.oid.clone(),
                    target_oid: target_oid.to_owned(),
                })
            })
            .collect();
        leads.push(InverseLead {
            query_material: source_materials[&lead.source].clone(),
            inverse_material: inverse_materials[&lead.inverse].clone(),
            query_equivalence: PatchEquivalence {
                representative_oid: representative_source.oid.clone(),
                member_count: source.members.len(),
                members: clone_members(&source.members),
                complete,
            },
            inverse_equivalence: PatchEquivalence {
                representative_oid: representative_inverse.oid.clone(),
                member_count: inverse.members.len(),
                members: clone_members(&inverse.members),
                complete,
            },
            explicit_reverts,
        });
    }

    Ok(Scan {
        report: FailureLeads {
            matched_count,
            returned_count,
            limit,
            eligible_count,
            checked_count,
            complete,
            indeterminate,
            leads,
        },
        equivalences,
    })
}

pub(super) fn empty(limit: usize) -> FailureLeads {
    FailureLeads {
        matched_count: 0,
        returned_count: 0,
        limit,
        eligible_count: 0,
        checked_count: 0,
        complete: true,
        indeterminate: Vec::new(),
        leads: Vec::new(),
    }
}

fn matching_sources(
    forward: &[u8],
    inverse: &[u8],
    by_forward: &HashMap<Vec<u8>, Vec<usize>>,
    by_inverse: &HashMap<Vec<u8>, Vec<usize>>,
) -> (Vec<usize>, Vec<usize>) {
    let mut equivalent = Vec::new();
    if let Some(groups) = by_forward.get(forward) {
        extend_unique(&mut equivalent, groups);
    }
    if let Some(groups) = by_inverse.get(inverse) {
        extend_unique(&mut equivalent, groups);
    }

    let mut inverse_groups = Vec::new();
    if let Some(groups) = by_inverse.get(forward) {
        extend_unique(&mut inverse_groups, groups);
    }
    if let Some(groups) = by_forward.get(inverse) {
        extend_unique(&mut inverse_groups, groups);
    }
    (equivalent, inverse_groups)
}

fn extend_unique(target: &mut Vec<usize>, values: &[usize]) {
    for value in values {
        if !target.contains(value) {
            target.push(*value);
        }
    }
}

fn add_member(
    members: &mut Vec<PatchGroupMember>,
    oids: &mut HashSet<String>,
    member: PatchGroupMember,
) {
    if oids.insert(member.oid.clone()) {
        members.push(member);
    }
}

fn member_for(member: &PatchGroupMember) -> PatchGroupMember {
    PatchGroupMember {
        oid: member.oid.clone(),
        subject: member.subject.clone(),
    }
}

fn clone_members(members: &[PatchGroupMember]) -> Vec<PatchGroupMember> {
    members.iter().map(member_for).collect()
}
