//! Strict patch relationships within the followups descendant and inspection scope.
use crate::{
    analysis::patch_relationship::{self, CompletePatch, PatchReader},
    app::AppError,
    git::Repository,
};

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
    seed_patch: Option<CompletePatch>,
    seed_fingerprint: Option<Vec<u8>>,
    seed_inverse_fingerprint: Option<Vec<u8>>,
    patches: Option<PatchReader<'a>>,
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
            seed_patch,
            seed_fingerprint,
            seed_inverse_fingerprint,
            patches: None,
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
            matcher.patches = Some(PatchReader::new(
                repository,
                cache_candidates.iter().map(String::as_str),
            )?);
        }
        Ok(matcher)
    }

    /// Checks every eligible commit reached by the existing followups traversal.
    pub(crate) fn check(&mut self, oid: &str) -> Result<Option<Relationship>, AppError> {
        let Some(seed) = self.seed_patch.as_ref().filter(|patch| !patch.is_empty()) else {
            return Ok(None);
        };
        self.checked_count += 1;
        let patches = self
            .patches
            .as_mut()
            .expect("nonempty seed with eligible candidates has a patch reader");
        let forward = self
            .seed_fingerprint
            .as_deref()
            .expect("nonempty seed fingerprint");
        let inverse = self
            .seed_inverse_fingerprint
            .as_deref()
            .expect("nonempty seed inverse fingerprint");
        let selected = patches.select(oid, |candidate_forward, candidate_inverse| {
            candidate_forward == forward
                || candidate_forward == inverse
                || candidate_inverse == forward
        })?;
        match selected {
            Ok(Some(patch)) => {
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
            Ok(None) => Ok(None),
            Err(reason) => {
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
