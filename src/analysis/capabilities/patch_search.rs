//! Complete-patch search; relationship certification stays shared.
use crate::{
    analysis::{
        patch_relationship::{self, CompletePatch, PatchReader},
        query::{Options, Outcome, PatchRelationSelection, QueryReport, scope},
    },
    app::AppError,
    cache,
    git::Repository,
};
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct Report {
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
    pub(crate) commit_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source: Option<CurrentPatchSource>,
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
pub(crate) struct CurrentPatchSource {
    pub(crate) kind: &'static str,
    pub(crate) staged: bool,
    pub(crate) head: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct PatchHunk {
    pub(crate) change_ordinal: i64,
    pub(crate) ordinal: i64,
    pub(crate) old_start: i64,
    pub(crate) old_lines: i64,
    pub(crate) new_start: i64,
    pub(crate) new_lines: i64,
    pub(crate) text: Vec<u8>,
}

fn material(
    oid: String,
    message: &[u8],
    patch: CompletePatch,
    relation: Option<&'static str>,
) -> PatchMaterial {
    PatchMaterial {
        commit_id: Some(oid),
        source: None,
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
        patch: patch_hunks(patch.hunks),
    }
}

fn patch_hunks(hunks: Vec<crate::git::Hunk>) -> Vec<PatchHunk> {
    hunks
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
        .collect()
}

fn current_material(
    source: CurrentPatchSource,
    patch: Result<CompletePatch, String>,
) -> PatchMaterial {
    match patch {
        Ok(patch) => PatchMaterial {
            commit_id: None,
            source: Some(source),
            subject: "Current change".to_owned(),
            integrity: if patch.is_empty() {
                "empty"
            } else {
                "complete"
            },
            reason: None,
            relation: None,
            normalization_version: patch_relationship::NORMALIZATION_VERSION,
            comparison_basis: Some(patch.comparison_basis),
            parent: patch.parent,
            paths: patch.paths,
            patch: patch_hunks(patch.hunks),
            files: patch.normalized,
        },
        Err(reason) => PatchMaterial {
            commit_id: None,
            source: Some(source),
            subject: "Current change".to_owned(),
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
    }
}

pub(crate) fn execute(
    revision: String,
    cutoff: Option<usize>,
    relation_selection: PatchRelationSelection,
    options: Options,
) -> Result<Outcome, AppError> {
    execute_input(
        PatchInput::Revision(revision),
        cutoff,
        relation_selection,
        options,
    )
}

pub(crate) fn execute_current(
    staged: bool,
    cutoff: Option<usize>,
    relation_selection: PatchRelationSelection,
    options: Options,
) -> Result<Outcome, AppError> {
    execute_input(
        PatchInput::Current { staged },
        cutoff,
        relation_selection,
        options,
    )
}

enum PatchInput {
    Revision(String),
    Current { staged: bool },
}

fn execute_input(
    input: PatchInput,
    cutoff: Option<usize>,
    relation_selection: PatchRelationSelection,
    options: Options,
) -> Result<Outcome, AppError> {
    let repository = Repository::discover()?;
    let branch_tips = repository.patch_branch_tips()?;
    let (query_oid, query_patch, current_source) = match input {
        PatchInput::Revision(revision) => {
            let oid = repository.resolve_commit(&revision)?;
            (
                Some(oid.clone()),
                patch_relationship::inspect(&repository, &oid),
                None,
            )
        }
        PatchInput::Current { staged } => {
            let current = repository.current_patch(staged)?;
            let source = CurrentPatchSource {
                kind: "current-change",
                staged: current.staged,
                head: current.head.clone(),
            };
            (
                None,
                patch_relationship::inspect_current(current),
                Some(source),
            )
        }
    };
    let mut refresh_tips: Vec<String> = branch_tips.iter().map(|(_, oid)| oid.clone()).collect();
    // Keep a selected historical query distinct from historical filters.
    refresh_tips.extend(query_oid.iter().cloned());
    refresh_tips.extend(
        current_source
            .as_ref()
            .and_then(|source| source.head.clone()),
    );
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
        .filter(|c| query_oid.as_ref().is_none_or(|oid| &c.oid != oid))
        .collect();
    let query_hashes = query_patch
        .as_ref()
        .ok()
        .filter(|patch| !patch.is_empty())
        .map(|patch| (patch.fingerprint(), patch.inverse_fingerprint()));
    let mut matches = Vec::new();
    let mut indeterminate = Vec::new();
    let mut checked = 0;
    if let Some((query_forward, query_inverse)) = &query_hashes {
        let mut patches = PatchReader::new(
            &repository,
            candidates
                .iter()
                .take(cutoff.unwrap_or(usize::MAX))
                .map(|candidate| candidate.oid.as_str()),
        )?;
        for candidate in &candidates {
            if cutoff.is_some_and(|limit| checked >= limit) {
                break;
            }
            checked += 1;
            let selected = patches.select(&candidate.oid, |forward, inverse| {
                (relation_selection.includes_equivalent() && forward == query_forward.as_slice())
                    || (relation_selection.includes_inverse()
                        && (forward == query_inverse.as_slice()
                            || inverse == query_forward.as_slice()))
            })?;
            match selected {
                Ok(Some(patch)) => {
                    let relation = query_patch.as_ref().ok().and_then(|query| {
                        if relation_selection.includes_equivalent() && query.equivalent(&patch) {
                            Some("equivalent")
                        } else if relation_selection.includes_inverse()
                            && query.inverse_equivalent(&patch)
                        {
                            Some("inverse")
                        } else {
                            None
                        }
                    });
                    if let Some(relation) = relation {
                        matches.push(material(
                            candidate.oid.clone(),
                            &candidate.message,
                            patch,
                            Some(relation),
                        ));
                    }
                }
                Ok(None) => {}
                Err(reason) => {
                    indeterminate.push(Indeterminate {
                        commit_id: candidate.oid.clone(),
                        reason,
                    });
                }
            }
        }
    }
    let query_is_empty = query_patch.as_ref().is_ok_and(CompletePatch::is_empty);
    let query_is_complete = query_patch.is_ok();
    let query_message = query_oid
        .as_deref()
        .map(|oid| session.commit_message(oid))
        .transpose()?
        .flatten()
        .unwrap_or_default();
    let query = match (query_oid, query_patch, current_source) {
        (Some(oid), Ok(patch), _) => material(oid, &query_message, patch, None),
        (Some(oid), Err(reason), _) => PatchMaterial {
            commit_id: Some(oid),
            source: None,
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
        (None, patch, Some(source)) => current_material(source, patch),
        (None, _, None) => unreachable!("current patch source was not supplied"),
    };
    // Keep each relationship together while retaining its within-group history order.
    matches.sort_by_key(|material| material.relation != Some("equivalent"));
    let matched_count = matches.len();
    matches.truncate(options.limit);
    let unexamined = if query_is_empty {
        0
    } else {
        candidates.len() - checked
    };
    let report = Report {
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
                && query_is_complete
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
