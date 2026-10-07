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

use std::collections::{HashMap, HashSet};
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

const MAX_TRACE_FIX_VERSION_HUNKS: usize = 64;
const MAX_TRACE_FIX_VERSION_TEXT_BYTES: usize = 2 * 1024;

#[derive(Serialize)]
pub(crate) struct TraceFixVersions {
    pub(crate) schema_version: u8,
    pub(crate) kind: &'static str,
    pub(crate) query: TraceFixVersionMaterial,
    pub(crate) matches: Vec<TraceFixVersionMaterial>,
    pub(crate) matched_count: usize,
    pub(crate) returned_count: usize,
    pub(crate) limit: usize,
    pub(crate) scope: Scope,
    pub(crate) warnings: Vec<String>,
}

#[derive(Serialize)]
pub(crate) struct TraceFixVersionMaterial {
    pub(crate) commit_id: String,
    pub(crate) subject: String,
    pub(crate) integrity: &'static str,
    pub(crate) reason: Option<String>,
    pub(crate) relation: Option<&'static str>,
    pub(crate) normalization_version: i64,
    pub(crate) comparison_basis: Option<&'static str>,
    pub(crate) parent: Option<String>,
    pub(crate) paths: Vec<Vec<u8>>,
    pub(crate) hunk_count: usize,
    pub(crate) hunks: Vec<TraceFixVersionHunk>,
    pub(crate) hunks_truncated: bool,
}

#[derive(Serialize)]
pub(crate) struct TraceFixVersionHunk {
    pub(crate) change_ordinal: i64,
    pub(crate) ordinal: i64,
    pub(crate) old_start: i64,
    pub(crate) old_lines: i64,
    pub(crate) new_start: i64,
    pub(crate) new_lines: i64,
    pub(crate) text: Option<String>,
    pub(crate) text_lossy: bool,
    pub(crate) text_truncated: bool,
}

pub(crate) fn trace_fix_versions(report: Report) -> TraceFixVersions {
    let mut remaining_hunks = MAX_TRACE_FIX_VERSION_HUNKS;
    let mut remaining_text_bytes = MAX_TRACE_FIX_VERSION_TEXT_BYTES;
    let query = trace_fix_version_material(
        report.query,
        &mut remaining_hunks,
        &mut remaining_text_bytes,
    );
    let matches = report
        .matches
        .into_iter()
        .map(|material| {
            trace_fix_version_material(material, &mut remaining_hunks, &mut remaining_text_bytes)
        })
        .collect();
    TraceFixVersions {
        schema_version: report.schema_version,
        kind: "trace-fix-versions",
        query,
        matches,
        matched_count: report.matched_count,
        returned_count: report.returned_count,
        limit: report.limit,
        scope: report.scope,
        warnings: report.warnings,
    }
}

fn trace_fix_version_material(
    material: PatchMaterial,
    remaining_hunks: &mut usize,
    remaining_text_bytes: &mut usize,
) -> TraceFixVersionMaterial {
    let hunk_count = material.patch.len();
    let hunks = material
        .patch
        .into_iter()
        .take(*remaining_hunks)
        .map(|hunk| {
            *remaining_hunks -= 1;
            let text_len = hunk.text.len().min(*remaining_text_bytes);
            let text_truncated = text_len < hunk.text.len();
            let text_lossy = std::str::from_utf8(&hunk.text[..text_len]).is_err();
            let text = (text_len > 0)
                .then(|| String::from_utf8_lossy(&hunk.text[..text_len]).into_owned());
            *remaining_text_bytes -= text_len;
            TraceFixVersionHunk {
                change_ordinal: hunk.change_ordinal,
                ordinal: hunk.ordinal,
                old_start: hunk.old_start,
                old_lines: hunk.old_lines,
                new_start: hunk.new_start,
                new_lines: hunk.new_lines,
                text,
                text_lossy,
                text_truncated,
            }
        })
        .collect::<Vec<_>>();
    TraceFixVersionMaterial {
        commit_id: material.commit_id,
        subject: material.subject,
        integrity: material.integrity,
        reason: material.reason,
        relation: material.relation,
        normalization_version: material.normalization_version,
        comparison_basis: material.comparison_basis,
        parent: material.parent,
        paths: material.paths,
        hunk_count,
        hunks_truncated: hunks.len() < hunk_count,
        hunks,
    }
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
    pub(crate) files: Vec<patch_relationship::FilePatch>,
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
        files: patch.normalized,
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
        // Cached normalization is reusable only while its source objects remain
        // locally available. Batch existence checks without re-reading every patch.
        let mut stored = HashMap::new();
        let mut objects = HashSet::new();
        for candidate in candidates.iter().take(cutoff.unwrap_or(usize::MAX)) {
            if let Some(record) = fingerprints.get(&candidate.oid)? {
                objects.extend(record.objects.lines().map(str::to_owned));
                stored.insert(candidate.oid.clone(), record);
            }
        }
        let missing: HashSet<_> = repository
            .missing_objects(&objects.into_iter().collect::<Vec<_>>())?
            .into_iter()
            .collect();
        for candidate in &candidates {
            if cutoff.is_some_and(|limit| checked >= limit) {
                break;
            }
            checked += 1;
            let stored = stored.remove(&candidate.oid);
            if stored
                .as_ref()
                .is_some_and(|record| record.objects.lines().any(|oid| missing.contains(oid)))
            {
                fingerprints.put(&candidate.oid, "indeterminate", None, &[])?;
                indeterminate.push(Indeterminate {
                    commit_id: candidate.oid.clone(),
                    reason: "cached patch source objects are unavailable".into(),
                });
                continue;
            }
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
                            &patch.objects,
                        )?;
                        inspected = Some(patch);
                        Some(hash)
                    }
                    Err(reason) => {
                        fingerprints.put(&candidate.oid, "indeterminate", None, &[])?;
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
                    fingerprints.put(&candidate.oid, "indeterminate", None, &[])?;
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
            files: Vec::new(),
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
