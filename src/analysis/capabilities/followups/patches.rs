//! Strict patch relationships within the followups descendant and inspection scope.
use crate::{
    analysis::patch_relationship::{self, CompletePatch},
    app::AppError,
    cache::{PatchFingerprint, PatchFingerprints},
    git::Repository,
};
use std::collections::{HashMap, HashSet};

pub(crate) struct MatchingReport {
    pub(crate) seed_patch: SeedPatch,
    pub(crate) eligible_count: usize,
    pub(crate) checked_count: usize,
    pub(crate) unexamined_count: usize,
    pub(crate) coverage_complete: bool,
    pub(crate) indeterminate: Vec<Indeterminate>,
}

pub(crate) struct SeedPatch {
    pub(crate) integrity: &'static str,
    pub(crate) reason: Option<String>,
    pub(crate) normalization_version: i64,
    pub(crate) comparison_basis: Option<&'static str>,
    pub(crate) parent: Option<String>,
    pub(crate) paths: Vec<Vec<u8>>,
}

pub(crate) struct Indeterminate {
    pub(crate) commit_id: String,
    pub(crate) reason: String,
}

#[derive(Clone)]
pub(crate) struct Relationship {
    pub(crate) relation: &'static str,
    pub(crate) seed_comparison_basis: &'static str,
    pub(crate) seed_parent: Option<String>,
    pub(crate) comparison_basis: &'static str,
    pub(crate) parent: Option<String>,
    pub(crate) paths: Vec<Vec<u8>>,
}

/// One query's seed patch, reusable candidate fingerprints, and coverage.
pub(crate) struct Matcher<'a> {
    repository: &'a Repository,
    seed_patch: Option<CompletePatch>,
    seed_fingerprint: Option<Vec<u8>>,
    seed_inverse_fingerprint: Option<Vec<u8>>,
    fingerprints: Option<PatchFingerprints>,
    stored: HashMap<String, PatchFingerprint>,
    missing_objects: HashSet<String>,
    eligible_count: usize,
    checked_count: usize,
    indeterminate: Vec<Indeterminate>,
    seed_material: SeedPatch,
}

impl<'a> Matcher<'a> {
    /// `cache_candidates` is bounded by `--max-commits`; further eligible
    /// descendants may be counted but are never fingerprinted pre-emptively.
    pub(crate) fn new(
        repository: &'a Repository,
        seed: &str,
        cache_candidates: &[String],
        eligible_count: usize,
    ) -> Result<Self, AppError> {
        let inspected = patch_relationship::inspect(repository, seed);
        let (seed_patch, seed_material, seed_fingerprint, seed_inverse_fingerprint) =
            match inspected {
                Ok(patch) => {
                    let integrity = if patch.is_empty() {
                        "empty"
                    } else {
                        "complete"
                    };
                    let material = SeedPatch {
                        integrity,
                        reason: None,
                        normalization_version: patch_relationship::NORMALIZATION_VERSION,
                        comparison_basis: Some(patch.comparison_basis),
                        parent: patch.parent.clone(),
                        paths: patch.paths.clone(),
                    };
                    if patch.is_empty() {
                        (Some(patch), material, None, None)
                    } else {
                        let forward = patch.fingerprint();
                        let inverse = patch.inverse_fingerprint();
                        (Some(patch), material, Some(forward), Some(inverse))
                    }
                }
                Err(reason) => (
                    None,
                    SeedPatch {
                        integrity: "indeterminate",
                        reason: Some(reason.clone()),
                        normalization_version: patch_relationship::NORMALIZATION_VERSION,
                        comparison_basis: None,
                        parent: None,
                        paths: Vec::new(),
                    },
                    None,
                    None,
                ),
            };
        let mut matcher = Self {
            repository,
            seed_patch,
            seed_fingerprint,
            seed_inverse_fingerprint,
            fingerprints: None,
            stored: HashMap::new(),
            missing_objects: HashSet::new(),
            eligible_count,
            checked_count: 0,
            indeterminate: Vec::new(),
            seed_material,
        };
        if matcher.seed_material.integrity == "indeterminate" {
            matcher.indeterminate.push(Indeterminate {
                commit_id: seed.to_owned(),
                reason: matcher
                    .seed_material
                    .reason
                    .clone()
                    .expect("indeterminate seed has a reason"),
            });
        }
        if matcher.seed_fingerprint.is_some() && !cache_candidates.is_empty() {
            let fingerprints = PatchFingerprints::open(
                &repository.common_dir,
                patch_relationship::NORMALIZATION_VERSION,
            )?;
            let mut objects = HashSet::new();
            for oid in cache_candidates {
                if let Some(record) = fingerprints.get(oid)? {
                    objects.extend(record.objects.lines().map(str::to_owned));
                    matcher.stored.insert(oid.clone(), record);
                }
            }
            matcher.missing_objects = repository
                .missing_objects(&objects.into_iter().collect::<Vec<_>>())?
                .into_iter()
                .collect();
            matcher.fingerprints = Some(fingerprints);
        }
        Ok(matcher)
    }

    /// Checks every eligible commit reached by the existing followups traversal.
    pub(crate) fn check(&mut self, oid: &str) -> Result<Option<Relationship>, AppError> {
        let Some(seed) = self.seed_patch.as_ref().filter(|patch| !patch.is_empty()) else {
            return Ok(None);
        };
        self.checked_count += 1;
        let fingerprints = self
            .fingerprints
            .as_ref()
            .expect("nonempty seed with eligible candidates has a fingerprint cache");
        let stored = self.stored.remove(oid);
        if stored.as_ref().is_some_and(|record| {
            record
                .objects
                .lines()
                .any(|object| self.missing_objects.contains(object))
        }) {
            fingerprints.put(oid, "indeterminate", None, None, &[])?;
            self.indeterminate.push(Indeterminate {
                commit_id: oid.to_owned(),
                reason: "cached patch source objects are unavailable".into(),
            });
            return Ok(None);
        }
        let mut inspected = None;
        let cached_hashes = stored
            .filter(|record| matches!(record.integrity.as_str(), "complete" | "empty"))
            .and_then(|record| Some((record.forward_fingerprint?, record.inverse_fingerprint?)));
        let (candidate_forward, candidate_inverse) = if let Some(hashes) = cached_hashes {
            hashes
        } else {
            match patch_relationship::inspect(self.repository, oid) {
                Ok(patch) => {
                    let forward = patch.fingerprint();
                    let inverse = patch.inverse_fingerprint();
                    fingerprints.put(
                        oid,
                        if patch.is_empty() {
                            "empty"
                        } else {
                            "complete"
                        },
                        Some(&forward),
                        Some(&inverse),
                        &patch.objects,
                    )?;
                    inspected = Some(patch);
                    (forward, inverse)
                }
                Err(reason) => {
                    fingerprints.put(oid, "indeterminate", None, None, &[])?;
                    self.indeterminate.push(Indeterminate {
                        commit_id: oid.to_owned(),
                        reason,
                    });
                    return Ok(None);
                }
            }
        };
        let equivalent_fingerprint_match = self
            .seed_fingerprint
            .as_deref()
            .is_some_and(|fingerprint| candidate_forward.as_slice() == fingerprint);
        let inverse_fingerprint_match =
            self.seed_inverse_fingerprint
                .as_deref()
                .is_some_and(|fingerprint| {
                    candidate_forward.as_slice() == fingerprint
                        || candidate_inverse.as_slice() == self.seed_fingerprint.as_deref().unwrap()
                });
        if !equivalent_fingerprint_match && !inverse_fingerprint_match {
            return Ok(None);
        }
        // Fingerprints narrow the candidates; complete normalized patches certify identity.
        let patch = inspected
            .map(Ok)
            .unwrap_or_else(|| patch_relationship::inspect(self.repository, oid));
        match patch {
            Ok(patch) => {
                let relation = if seed.equivalent(&patch) {
                    Some("equivalent")
                } else if seed.inverse_equivalent(&patch) {
                    Some("inverse")
                } else {
                    None
                };
                Ok(relation.map(|relation| Relationship {
                    relation,
                    seed_comparison_basis: seed.comparison_basis,
                    seed_parent: seed.parent.clone(),
                    comparison_basis: patch.comparison_basis,
                    parent: patch.parent,
                    paths: patch.paths,
                }))
            }
            Err(reason) => {
                fingerprints.put(oid, "indeterminate", None, None, &[])?;
                self.indeterminate.push(Indeterminate {
                    commit_id: oid.to_owned(),
                    reason,
                });
                Ok(None)
            }
        }
    }

    pub(crate) fn report(&self, history_coverage_complete: bool) -> MatchingReport {
        let query_empty = self.seed_material.integrity == "empty";
        let unexamined_count = if query_empty {
            0
        } else {
            self.eligible_count.saturating_sub(self.checked_count)
        };
        MatchingReport {
            seed_patch: SeedPatch {
                integrity: self.seed_material.integrity,
                reason: self.seed_material.reason.clone(),
                normalization_version: self.seed_material.normalization_version,
                comparison_basis: self.seed_material.comparison_basis,
                parent: self.seed_material.parent.clone(),
                paths: self.seed_material.paths.clone(),
            },
            eligible_count: self.eligible_count,
            checked_count: self.checked_count,
            unexamined_count,
            coverage_complete: history_coverage_complete
                && unexamined_count == 0
                && self.indeterminate.is_empty()
                && self.seed_material.integrity != "indeterminate",
            indeterminate: self
                .indeterminate
                .iter()
                .map(|item| Indeterminate {
                    commit_id: item.commit_id.clone(),
                    reason: item.reason.clone(),
                })
                .collect(),
        }
    }
}
