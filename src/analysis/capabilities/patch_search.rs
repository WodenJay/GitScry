//! Existing-commit patch search; relationship certification stays shared.
use crate::{
    analysis::{
        patch_relationship::{self, CompletePatch},
        query::{Options, Outcome, QueryReport, scope},
    },
    app::AppError,
    cache::{self, PatchFingerprints},
    git::Repository,
};
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct Report {
    pub(crate) schema_version: u8,
    pub(crate) kind: &'static str,
    pub(crate) query: PatchMaterial,
    pub(crate) matches: Vec<PatchMaterial>,
    pub(crate) matched_count: usize,
    pub(crate) returned_count: usize,
    pub(crate) limit: usize,
    pub(crate) scope: Scope,
    pub(crate) warnings: Vec<String>,
}
#[derive(Serialize)]
pub(crate) struct Scope {
    pub(crate) branch_tips: Vec<(String, String)>,
    pub(crate) from_rev: Option<String>,
    pub(crate) to_rev: String,
    pub(crate) since: Option<String>,
    pub(crate) until: Option<String>,
    pub(crate) coverage_complete: bool,
    pub(crate) eligible_count: usize,
    pub(crate) checked_count: usize,
    pub(crate) unexamined_count: usize,
    pub(crate) indeterminate: Vec<Indeterminate>,
    pub(crate) max_patch_checks: Option<usize>,
}
#[derive(Serialize)]
pub(crate) struct Indeterminate {
    pub(crate) commit_id: String,
    pub(crate) reason: String,
}
#[derive(Serialize)]
pub(crate) struct PatchMaterial {
    pub(crate) commit_id: String,
    pub(crate) subject: String,
    pub(crate) integrity: &'static str,
    pub(crate) reason: Option<String>,
    pub(crate) relation: Option<&'static str>,
    pub(crate) normalization_version: i64,
    pub(crate) comparison_basis: Option<&'static str>,
    pub(crate) parent: Option<String>,
    pub(crate) paths: Vec<Vec<u8>>,
    pub(crate) patch: Vec<PatchHunk>,
}
#[derive(Serialize)]
pub(crate) struct PatchHunk {
    change_ordinal: i64,
    ordinal: i64,
    old_start: i64,
    old_lines: i64,
    new_start: i64,
    new_lines: i64,
    text: Vec<u8>,
}

fn material(
    oid: String,
    message: &[u8],
    patch: CompletePatch,
    relation: Option<&'static str>,
) -> PatchMaterial {
    PatchMaterial {
        commit_id: oid,
        subject: String::from_utf8_lossy(message)
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned(),
        integrity: if patch.is_empty() {
            "empty"
        } else {
            "complete"
        },
        reason: None,
        relation,
        normalization_version: patch_relationship::NORMALIZATION_VERSION,
        comparison_basis: Some(patch.comparison_basis),
        parent: patch.parent,
        paths: patch.paths,
        patch: patch
            .hunks
            .into_iter()
            .map(|h| PatchHunk {
                change_ordinal: h.change_ordinal,
                ordinal: h.ordinal,
                old_start: h.old_start,
                old_lines: h.old_lines,
                new_start: h.new_start,
                new_lines: h.new_lines,
                text: h.text,
            })
            .collect(),
    }
}

pub(crate) fn execute(
    revision: String,
    cutoff: Option<usize>,
    options: Options,
) -> Result<Outcome, AppError> {
    let repository = Repository::discover()?;
    let query_oid = repository.resolve_commit(&revision)?;
    let branch_tips = repository.patch_branch_tips()?;
    let mut refresh_tips: Vec<String> = branch_tips.iter().map(|(_, oid)| oid.clone()).collect();
    // Keep the selected query distinct from historical filters.
    refresh_tips.push(query_oid.clone());
    if let Some(revision) = &options.scope.to_rev {
        refresh_tips.push(repository.resolve_commit(revision)?);
    }
    refresh_tips.sort();
    refresh_tips.dedup();
    let tips: Vec<_> = refresh_tips.iter().map(String::as_str).collect();
    let session = cache::refresh_query_targets(&repository, &tips)?;
    let resolved = scope::resolve_for_patch(&session, &repository, options.scope, &branch_tips)?;
    let candidates: Vec<_> = session
        .commits_scoped(&resolved.filter)?
        .into_iter()
        .rev()
        .filter(|c| c.oid != query_oid)
        .collect();
    let fingerprints = PatchFingerprints::open(
        &repository.common_dir,
        patch_relationship::NORMALIZATION_VERSION,
    )?;
    let query_patch = patch_relationship::inspect(&repository, &query_oid);
    let query_hash = query_patch
        .as_ref()
        .ok()
        .filter(|patch| !patch.is_empty())
        .map(CompletePatch::fingerprint);
    let mut matches = Vec::new();
    let mut indeterminate = Vec::new();
    let mut checked = 0;
    if let Some(query_hash) = &query_hash {
        for candidate in &candidates {
            if cutoff.is_some_and(|limit| checked >= limit) {
                break;
            }
            checked += 1;
            let stored = fingerprints.get(&candidate.oid)?;
            let mut inspected = None;
            let hash = if let Some(stored) =
                stored.filter(|stored| matches!(stored.integrity.as_str(), "complete" | "empty"))
            {
                stored.fingerprint
            } else {
                match patch_relationship::inspect(&repository, &candidate.oid) {
                    Ok(patch) => {
                        let hash = patch.fingerprint();
                        fingerprints.put(
                            &candidate.oid,
                            if patch.is_empty() {
                                "empty"
                            } else {
                                "complete"
                            },
                            Some(&hash),
                        )?;
                        inspected = Some(patch);
                        Some(hash)
                    }
                    Err(reason) => {
                        fingerprints.put(&candidate.oid, "indeterminate", None)?;
                        indeterminate.push(Indeterminate {
                            commit_id: candidate.oid.clone(),
                            reason,
                        });
                        None
                    }
                }
            };
            if hash.as_ref() != Some(query_hash) {
                continue;
            }
            // Every hash candidate is certified with full material, including on
            // repeated queries. A fingerprint is never relationship proof.
            let patch = inspected
                .map(Ok)
                .unwrap_or_else(|| patch_relationship::inspect(&repository, &candidate.oid));
            match patch {
                Ok(patch)
                    if query_patch
                        .as_ref()
                        .is_ok_and(|query| query.equivalent(&patch)) =>
                {
                    matches.push(material(
                        candidate.oid.clone(),
                        &candidate.message,
                        patch,
                        Some("equivalent"),
                    ));
                }
                Ok(_) => {}
                Err(reason) => {
                    fingerprints.put(&candidate.oid, "indeterminate", None)?;
                    indeterminate.push(Indeterminate {
                        commit_id: candidate.oid.clone(),
                        reason,
                    });
                }
            }
        }
    }
    let query_message = session.commit_message(&query_oid)?.unwrap_or_default();
    let query = match query_patch {
        Ok(patch) => material(query_oid.clone(), &query_message, patch, None),
        Err(reason) => PatchMaterial {
            commit_id: query_oid,
            subject: String::from_utf8_lossy(&query_message)
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned(),
            integrity: "indeterminate",
            reason: Some(reason),
            relation: None,
            normalization_version: patch_relationship::NORMALIZATION_VERSION,
            comparison_basis: None,
            parent: None,
            paths: Vec::new(),
            patch: Vec::new(),
        },
    };
    let matched_count = matches.len();
    matches.truncate(options.limit);
    let unexamined = if query.integrity == "empty" {
        0
    } else {
        candidates.len() - checked
    };
    let report = Report {
        schema_version: 1,
        kind: "patch-search",
        query,
        returned_count: matches.len(),
        matches,
        matched_count,
        limit: options.limit,
        scope: Scope {
            branch_tips,
            from_rev: resolved.report.from_rev,
            to_rev: resolved.report.to_rev,
            since: resolved.report.since,
            until: resolved.report.until,
            coverage_complete: resolved.report.coverage_complete
                && indeterminate.is_empty()
                && unexamined == 0,
            eligible_count: candidates.len(),
            checked_count: checked,
            unexamined_count: unexamined,
            indeterminate,
            max_patch_checks: cutoff,
        },
        warnings: Vec::new(),
    };
    Ok(Outcome {
        progress: session.progress().to_vec(),
        warnings: session.warnings().to_vec(),
        report: QueryReport::PatchSearch(report),
    })
}
