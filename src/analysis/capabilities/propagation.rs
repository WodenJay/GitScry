//! Containment and whole-commit patch equivalence across explicit target refs.
//!
//! Exact reachability is checked first, which is always authoritative. When the
//! source commit is not reachable from a target, the source's whole-commit patch
//! is compared against the ordinary commits reachable from that target using
//! `git patch-id --verbatim`. A target ends `not_found` only when that search
//! completed; otherwise it is `indeterminate` with the concrete gap.

use std::collections::HashSet;

use serde::Serialize;

use crate::analysis::query::{Outcome, QueryReport};
use crate::git::{PatchLookup, PatchSpec};
use crate::{app::AppError, cache, git};

const REASON_HISTORY_INCOMPLETE: &str = "target history is incomplete in the prepared cache; patch equivalence could not be fully inspected";
const REASON_SOURCE_MERGE: &str = "source is a merge commit; whole-commit patch equivalence is only supported for non-merge commits";
const REASON_SOURCE_NO_PATCH: &str =
    "source commit has no whole-commit patch to compare against target history";
const REASON_SOURCE_PATCH_UNREADABLE: &str =
    "source patch could not be read from the local object store";
const REASON_TARGET_PATCH_UNREADABLE: &str = "some target commits could not be read for patch equivalence; the local object store is missing objects";
const MATCHING_METHOD_REACHABILITY: &str = "exact_reachability";
const MATCHING_METHOD_PATCH_ID: &str = "patch-id --verbatim";

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
    /// Commits whose whole-commit patch matches the source; empty unless status is equivalent.
    pub(crate) equivalents: Vec<String>,
    /// Identifier shared by the source and equivalent commits, when applicable.
    pub(crate) patch_identifier: Option<String>,
    /// How a positive result was established, when a positive result exists.
    pub(crate) matching_method: Option<String>,
    /// Concrete reason the inspection did not establish an exact answer.
    pub(crate) reason: Option<String>,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
    Contained,
    Equivalent,
    NotFound,
    Indeterminate,
}

/// The source commit's whole-commit patch, or why it cannot take part in a search.
enum SourcePatch {
    Identifier(String),
    Merge,
    NoPatch,
    Unreadable,
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
        let mut seen = HashSet::new();
        resolved
            .iter()
            .filter(|(_, oid)| seen.insert(oid.clone()))
            .map(|(_, oid)| oid.as_str())
            .collect()
    };
    let session = cache::refresh_query_targets(&repository, &unique_tips)?;
    let progress = session.progress().to_vec();
    let warnings = session.warnings().to_vec();
    let cached = session.commit_oids()?;
    for (_, oid) in &resolved {
        session.require_revision(oid)?;
    }

    let source_patch = source_patch(&repository, &source_oid)?;
    let shallow_boundaries = repository.shallow_boundaries()?;

    let mut results = Vec::with_capacity(resolved.len());
    let mut coverage_complete = true;
    for (target_ref, target_oid) in &resolved {
        let inspection = inspect_target(
            &repository,
            &session,
            &cached,
            &shallow_boundaries,
            &source_oid,
            &source_patch,
            target_ref,
            target_oid,
        )?;
        coverage_complete &= inspection.complete;
        results.push(inspection.result);
    }
    let report = Report {
        schema_version: 2,
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

struct Inspection {
    result: TargetResult,
    complete: bool,
}

#[allow(clippy::too_many_arguments)]
fn inspect_target(
    repository: &git::Repository,
    session: &cache::QuerySession,
    cached: &HashSet<String>,
    shallow_boundaries: &[String],
    source_oid: &str,
    source_patch: &SourcePatch,
    target_ref: &str,
    target_oid: &str,
) -> Result<Inspection, AppError> {
    let (reachable, traversal_reason) = match repository.reachable_commit_parents(target_oid) {
        Ok(parents) => (parents.into_keys().collect::<HashSet<_>>(), None),
        Err(error) => (
            session.ancestors(target_oid)?,
            Some(history_traversal_reason(&error)),
        ),
    };
    let shallow_reachable = shallow_boundaries
        .iter()
        .any(|boundary| reachable.contains(boundary));
    let uncached = reachable
        .iter()
        .filter(|oid| !cached.contains(*oid))
        .count();
    let mut history_incomplete = traversal_reason.is_some() || shallow_reachable || uncached > 0;
    if reachable.contains(source_oid) {
        // Exact reachability is authoritative; a positive result is reported even
        // when other target history remains unavailable.
        let reason = if let Some(traversal_reason) = &traversal_reason {
            Some(format!("{REASON_HISTORY_INCOMPLETE}; {traversal_reason}"))
        } else {
            history_incomplete.then(|| REASON_HISTORY_INCOMPLETE.to_owned())
        };
        let result = TargetResult {
            target_ref: target_ref.to_owned(),
            target_oid: target_oid.to_owned(),
            source_oid: source_oid.to_owned(),
            status: Status::Contained,
            contained_by: vec![source_oid.to_owned()],
            equivalents: Vec::new(),
            patch_identifier: None,
            matching_method: Some(MATCHING_METHOD_REACHABILITY.to_owned()),
            reason,
        };
        return Ok(Inspection {
            result,
            complete: !history_incomplete,
        });
    }

    // Not reachable: search the target's ordinary reachable commits for a
    // whole-commit patch equivalent to the source.
    let mut reasons: Vec<String> = Vec::new();
    if history_incomplete {
        reasons.push(REASON_HISTORY_INCOMPLETE.to_owned());
        if let Some(traversal_reason) = &traversal_reason {
            reasons.push(traversal_reason.clone());
        }
    }
    let identifier = match source_patch {
        SourcePatch::Identifier(identifier) => Some(identifier.as_str()),
        SourcePatch::Merge => {
            reasons.insert(0, REASON_SOURCE_MERGE.to_owned());
            None
        }
        SourcePatch::NoPatch => {
            reasons.insert(0, REASON_SOURCE_NO_PATCH.to_owned());
            None
        }
        SourcePatch::Unreadable => {
            reasons.insert(0, REASON_SOURCE_PATCH_UNREADABLE.to_owned());
            None
        }
    };
    let Some(identifier) = identifier else {
        return Ok(indeterminate(target_ref, target_oid, source_oid, reasons));
    };

    let order = if traversal_reason.is_some() {
        cached_history_order(session, target_oid)?
    } else {
        match repository.reachable_commits_in_history_order(target_oid) {
            Ok(order) => order,
            Err(error) => {
                if !history_incomplete {
                    reasons.push(REASON_HISTORY_INCOMPLETE.to_owned());
                }
                history_incomplete = true;
                reasons.push(history_traversal_reason(&error));
                cached_history_order(session, target_oid)?
            }
        }
    };
    let mut patches_unreadable = false;
    let mut specs = Vec::new();
    for oid in order.iter().filter(|oid| cached.contains(*oid)) {
        match repository.commit_parents(oid) {
            Ok(commit_parents) if commit_parents.len() > 1 => {}
            Ok(commit_parents) => {
                specs.push(PatchSpec::new(oid.clone(), commit_parents.first().cloned()))
            }
            Err(_) => patches_unreadable = true,
        }
    }
    let lookups = repository.commit_patch_ids(&specs)?;
    let mut equivalents = Vec::new();
    for (oid, lookup) in &lookups {
        match lookup {
            PatchLookup::Identifier(found) if found == identifier => equivalents.push(oid.clone()),
            PatchLookup::Unreadable => patches_unreadable = true,
            _ => {}
        }
    }
    if patches_unreadable {
        reasons.push(REASON_TARGET_PATCH_UNREADABLE.to_owned());
    }
    let complete = !history_incomplete && !patches_unreadable;

    if !equivalents.is_empty() {
        // A positive match is reported as equivalent regardless of remaining
        // coverage gaps; incompleteness is carried separately.
        let result = TargetResult {
            target_ref: target_ref.to_owned(),
            target_oid: target_oid.to_owned(),
            source_oid: source_oid.to_owned(),
            status: Status::Equivalent,
            contained_by: Vec::new(),
            equivalents,
            patch_identifier: Some(identifier.to_owned()),
            matching_method: Some(MATCHING_METHOD_PATCH_ID.to_owned()),
            reason: (!reasons.is_empty()).then(|| reasons.join("; ")),
        };
        return Ok(Inspection { result, complete });
    }

    if complete {
        let result = TargetResult {
            target_ref: target_ref.to_owned(),
            target_oid: target_oid.to_owned(),
            source_oid: source_oid.to_owned(),
            status: Status::NotFound,
            contained_by: Vec::new(),
            equivalents: Vec::new(),
            patch_identifier: None,
            matching_method: None,
            reason: None,
        };
        return Ok(Inspection { result, complete });
    }

    Ok(indeterminate(target_ref, target_oid, source_oid, reasons))
}

fn history_traversal_reason(error: &AppError) -> String {
    let detail = error
        .to_string()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    format!("target history could not be traversed: {detail}")
}

fn cached_history_order(
    session: &cache::QuerySession,
    target_oid: &str,
) -> Result<Vec<String>, AppError> {
    session.commits_reachable_from(target_oid)
}

fn indeterminate(
    target_ref: &str,
    target_oid: &str,
    source_oid: &str,
    reasons: Vec<String>,
) -> Inspection {
    let reason = if reasons.is_empty() {
        "patch equivalence could not be completed".to_owned()
    } else {
        reasons.join("; ")
    };
    Inspection {
        result: TargetResult {
            target_ref: target_ref.to_owned(),
            target_oid: target_oid.to_owned(),
            source_oid: source_oid.to_owned(),
            status: Status::Indeterminate,
            contained_by: Vec::new(),
            equivalents: Vec::new(),
            patch_identifier: None,
            matching_method: None,
            reason: Some(reason),
        },
        complete: false,
    }
}

fn source_patch(repository: &git::Repository, source_oid: &str) -> Result<SourcePatch, AppError> {
    let parents = repository.commit_parents(source_oid)?;
    if parents.len() > 1 {
        return Ok(SourcePatch::Merge);
    }
    let spec = PatchSpec::new(source_oid.to_owned(), parents.first().cloned());
    let lookups = repository.commit_patch_ids(&[spec])?;
    Ok(match lookups.into_iter().next().map(|(_, lookup)| lookup) {
        Some(PatchLookup::Identifier(identifier)) => SourcePatch::Identifier(identifier),
        Some(PatchLookup::NoPatch) => SourcePatch::NoPatch,
        Some(PatchLookup::Unreadable) | None => SourcePatch::Unreadable,
    })
}
