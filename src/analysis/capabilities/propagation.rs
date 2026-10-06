//! Exact commit reachability from explicit target refs, with honest gaps.
use serde::Serialize;

use crate::analysis::query::{Outcome, QueryReport};
use crate::{app::AppError, cache, git};

const REASON_PATCH_CHECK_PENDING: &str = "patch equivalence has not been checked for this target";
const REASON_HISTORY_INCOMPLETE: &str =
    "target history is incomplete in the prepared cache; reachability was not fully inspected";
const REASON_SOURCE_UNCACHED: &str =
    "source commit is not cached; containment could not be inspected";

#[derive(Serialize)]
pub(crate) struct Report {
    pub(crate) schema_version: u8,
    pub(crate) source: String,
    pub(crate) targets: Vec<TargetResult>,
    pub(crate) coverage_complete: bool,
    pub(crate) warnings: Vec<String>,
}

#[derive(Serialize)]
pub(crate) struct TargetResult {
    pub(crate) target_ref: String,
    pub(crate) target_oid: String,
    pub(crate) source_oid: String,
    pub(crate) status: Status,
    /// Exact commits that establish containment; empty unless status is contained.
    pub(crate) contained_by: Vec<String>,
    /// Concrete reason the inspection did not establish an exact answer.
    pub(crate) reason: Option<String>,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
    Contained,
    Indeterminate,
}

pub(crate) fn execute(source: &str, targets: &[String]) -> Result<Outcome, AppError> {
    if targets.is_empty() {
        return Err(AppError::input(
            "at least one --to target is required; repeat --to REF for each target",
        ));
    }
    let repository = git::Repository::discover()?;
    let source_oid = repository.resolve_commit(source)?;
    let resolved: Vec<(String, String)> = targets
        .iter()
        .map(|target| {
            repository
                .resolve_commit(target)
                .map(|oid| (target.clone(), oid))
        })
        .collect::<Result<_, _>>()?;
    let unique_tips: Vec<&str> = {
        let mut seen = std::collections::HashSet::new();
        resolved
            .iter()
            .filter(|(_, oid)| seen.insert(oid.clone()))
            .map(|(_, oid)| oid.as_str())
            .collect()
    };
    let session = cache::refresh_query_targets(&repository, &unique_tips)?;
    let progress = session.progress().to_vec();
    let mut warnings = session.warnings().to_vec();
    let cached = session.commit_oids()?;
    let source_cached = cached.contains(&source_oid);
    for (_, oid) in &resolved {
        session.require_revision(oid)?;
    }
    if !source_cached {
        warnings.push(format!(
            "warning: source commit {source_oid} is outside the prepared cache; containment could not be inspected"
        ));
    }
    let mut results = Vec::with_capacity(resolved.len());
    let mut coverage_complete = true;
    for (target_ref, target_oid) in &resolved {
        let ancestors = session.ancestors(target_oid)?;
        let missing = ancestors.difference(&cached).count();
        let mut reasons: Vec<&'static str> = Vec::new();
        if !source_cached {
            reasons.push(REASON_SOURCE_UNCACHED);
        }
        if missing > 0 {
            reasons.push(REASON_HISTORY_INCOMPLETE);
        }
        if !source_cached || missing > 0 {
            coverage_complete = false;
        }
        if ancestors.contains(&source_oid) {
            // An exact positive result survives incomplete coverage: the source
            // OID itself was resolved and observed reachable from this target.
            let contained_by = vec![source_oid.clone()];
            let reason = reasons.first().map(|reason| reason.to_string());
            results.push(TargetResult {
                target_ref: target_ref.clone(),
                target_oid: target_oid.clone(),
                source_oid: source_oid.clone(),
                status: Status::Contained,
                contained_by,
                reason,
            });
        } else {
            // Exact non-reachability is not a completed negative search until
            // patch equivalence is checked.
            reasons.push(REASON_PATCH_CHECK_PENDING);
            let reason = reasons.join("; ");
            results.push(TargetResult {
                target_ref: target_ref.clone(),
                target_oid: target_oid.clone(),
                source_oid: source_oid.clone(),
                status: Status::Indeterminate,
                contained_by: Vec::new(),
                reason: Some(reason),
            });
        }
    }
    warnings.sort();
    warnings.dedup();
    let report = Report {
        schema_version: 1,
        source: source.to_owned(),
        targets: results,
        coverage_complete,
        warnings,
    };
    Ok(Outcome {
        progress,
        warnings: Vec::new(),
        report: QueryReport::Propagation(report),
    })
}
