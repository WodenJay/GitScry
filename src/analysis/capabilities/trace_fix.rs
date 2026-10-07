use std::collections::{HashMap, HashSet};

use crate::{
    app::AppError,
    cache::QuerySession,
    git::{DeletedLine, TraceFixTarget},
};

use crate::analysis::capabilities::patch_search::{PatchMaterial, Report as PatchSearchReport};

use super::super::provenance;
use super::super::retrieval;
use super::super::{Citation, Confidence, Detail, Material, Report, ReportKind, patch};
use crate::analysis::query::{Context, Options, Outcome, QueryReport};
use crate::git::Repository;

use serde::Serialize;

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
    pub(crate) scope: crate::analysis::capabilities::patch_search::Scope,
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

fn trace_fix_versions(report: PatchSearchReport) -> TraceFixVersions {
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
pub(in crate::analysis) fn execute(
    revision: String,
    paths: Vec<String>,
    options: Options,
) -> Result<Outcome, AppError> {
    let repository = Repository::discover()?;
    let version_limit = options.limit;
    let include_patch = options.patch;
    let (context, target) = Context::prepare_target(
        &repository,
        Some(revision.as_str()),
        options.scope,
        |revision| repository.pin_trace_fix(revision, &paths),
    )?;
    let mut reachable = context.session.ancestors(&target.revision)?;
    context.intersect(&target.revision, &mut reachable)?;
    let mut report = run(
        &context.session,
        &target,
        &reachable,
        version_limit,
        context.scope.is_some(),
    )?;
    if include_patch {
        patch::attach_trace_fix_patch_excerpts(&context.session, &mut report)?;
    }
    let mut outcome = context.finish(QueryReport::Analysis(report));
    let versions_outcome = crate::analysis::capabilities::patch_search::execute(
        target.revision.clone(),
        None,
        Options {
            limit: version_limit,
            patch: false,
            scope: crate::analysis::query::SearchScopeOptions::default(),
        },
    )?;
    let QueryReport::PatchSearch(versions) = versions_outcome.report else {
        unreachable!("patch-search execution returns a patch-search report")
    };
    let QueryReport::Analysis(report) = &mut outcome.report else {
        unreachable!("trace-fix assembles an analysis report")
    };
    report.fix_versions = Some(Box::new(trace_fix_versions(versions)));
    outcome.progress.extend(versions_outcome.progress);
    outcome.warnings.extend(versions_outcome.warnings);
    Ok(outcome)
}

pub(crate) struct TraceFixPatchAnchor {
    pub(crate) line: usize,
    pub(crate) paths: Vec<Vec<u8>>,
}

pub(crate) struct TraceFixDetail {
    pub(crate) role: &'static str,
    pub(crate) fix_revision: String,
    pub(crate) parent_revision: Option<String>,
    pub(crate) line: Option<usize>,
    pub(crate) patch_anchors: Vec<TraceFixPatchAnchor>,
}

type PathHistories = HashMap<Vec<u8>, HashMap<String, retrieval::HistoryCommit>>;
struct Evidence {
    oid: String,
    subject: String,
    commit_time: i64,
    parent_count: usize,
    boundary: bool,
    moved: bool,
    shared_reference: Option<String>,
    failure: Option<(String, String)>,
    movement: Option<(String, String)>,
    paths: Vec<Vec<u8>>,
    lines: Vec<usize>,
    patch_anchors: Vec<TraceFixPatchAnchor>,
}

fn run(
    session: &QuerySession,
    target: &TraceFixTarget,
    reachable: &HashSet<String>,
    limit: usize,
    scope_applied: bool,
) -> Result<Report, AppError> {
    let histories = candidate_history(session, &target.deleted_lines, reachable)?;
    let references = message_references(&target.fix.message);
    let pre_fix = target
        .parent
        .as_deref()
        .map(|parent| session.ancestors(parent))
        .transpose()?
        .unwrap_or_default();
    let evidence = collect_evidence(
        &target.deleted_lines,
        target.shallow,
        &histories,
        &references,
        &pre_fix,
        &target.fix.oid,
    );
    let (fix_subject, fix_body) = retrieval::message_parts(&target.fix.message);
    let observed_failure = provenance::stated_reason(&fix_subject, &fix_body)
        .or_else(|| provenance::revert_reason(&fix_subject, &fix_body))
        .is_some();

    let mut ranked = evidence
        .into_iter()
        .map(|evidence| {
            let confidence = if evidence.boundary || target.fix.parents.len() > 1 {
                Confidence::Low
            } else if evidence.moved || evidence.parent_count > 1 || !observed_failure {
                Confidence::Medium
            } else {
                Confidence::High
            };
            let mut basis =
                vec!["fix-parent deleted-line blame identifies this introducing change".to_owned()];
            if evidence.lines.len() > 1 {
                basis.push(format!("deleted lines: {}", evidence.lines.len()));
            }
            if observed_failure {
                basis.push("fix commit message records observed failure context".to_owned());
            } else {
                basis.push(
                    "observed failure context is unavailable from local commit messages".to_owned(),
                );
            }
            if !references.is_empty() {
                basis.push(format!("fix message references: {}", references.join(", ")));
            }
            if let Some(reference) = evidence.shared_reference {
                basis.push(format!(
                    "message reference {reference} corroborates the lineage"
                ));
            }
            if let Some((_, _)) = &evidence.failure {
                basis.push("observed failure commit shares a fix message reference".to_owned());
            }
            if evidence.moved {
                basis.push(
                    "code movement: anchored path history followed a rename or copy".to_owned(),
                );
            }
            if target.fix.parents.len() > 1 {
                basis.push("fix is a merge; deleted lines use the ordered first parent".to_owned());
            }
            if evidence.parent_count > 1 {
                basis.push("merge lineage uses the ordered first parent".to_owned());
            }
            if evidence.boundary {
                basis.push("shallow history boundary".to_owned());
            }
            let mut citations = vec![
                Citation::new(evidence.oid.clone(), evidence.subject.clone())
                    .noting("introducing change"),
            ];
            if let Some((failure_oid, failure_subject)) = evidence.failure.clone() {
                citations
                    .push(Citation::new(failure_oid, failure_subject).noting("observed failure"));
            }
            if let Some((movement_oid, movement_subject)) = evidence.movement.clone() {
                citations
                    .push(Citation::new(movement_oid, movement_subject).noting("code movement"));
            }
            let fix_citation = Citation::new(target.fix.oid.clone(), fix_subject.clone());
            citations.push(if observed_failure {
                fix_citation.noting("observed failure and fix")
            } else {
                fix_citation.noting("fix")
            });
            let first_line = evidence.lines.first().copied();
            retrieval::Ranked {
                score: if evidence.boundary { 70.0 } else { 90.0 },
                commit_time: evidence.commit_time,
                oid: evidence.oid.clone(),
                value: Material {
                    subject: if evidence.subject.is_empty() {
                        "Introducing commit (subject unavailable)".to_owned()
                    } else {
                        evidence.subject
                    },
                    paths: evidence.paths,
                    confidence,
                    basis,
                    citations,
                    detail: Some(Detail::TraceFix(TraceFixDetail {
                        role: "introducing change",
                        fix_revision: target.revision.clone(),
                        parent_revision: target.parent.clone(),
                        line: first_line,
                        patch_anchors: evidence.patch_anchors,
                    })),
                    patch: None,
                },
            }
        })
        .collect::<Vec<_>>();

    retrieval::sort(&mut ranked);
    let matched_count = ranked.len();
    let mut materials = ranked
        .into_iter()
        .take(limit)
        .map(|ranked| ranked.value)
        .collect::<Vec<_>>();
    retrieval::assign_citations(&mut materials);
    let mut report = super::super::report(ReportKind::TraceFix, materials, matched_count, limit);
    report.notices.extend(target.warnings.iter().cloned());
    report
        .notices
        .push("Remote context unavailable from local history.".to_owned());
    if !scope_applied && !target.deleted_lines.is_empty() && matched_count == 0 {
        report.notices.push(
            "warning: no introducing commit was found in cached history reachable from this fix; run `gitscry index` if relevant history is missing.".to_owned(),
        );
    }
    Ok(report)
}

fn candidate_history(
    session: &QuerySession,
    deleted_lines: &[DeletedLine],
    reachable: &HashSet<String>,
) -> Result<PathHistories, AppError> {
    let mut histories = PathHistories::new();
    for deleted in deleted_lines {
        if let std::collections::hash_map::Entry::Vacant(entry) =
            histories.entry(deleted.path.clone())
        {
            let history = session
                .path_history(&deleted.path, reachable)?
                .into_iter()
                .map(|commit| (commit.oid.clone(), commit))
                .collect();
            entry.insert(history);
        }
    }
    Ok(histories)
}

fn collect_evidence(
    deleted_lines: &[DeletedLine],
    shallow: bool,
    histories: &PathHistories,
    references: &[String],
    pre_fix: &HashSet<String>,
    fix_oid: &str,
) -> Vec<Evidence> {
    let mut by_oid = HashMap::<String, Evidence>::new();
    for deleted in deleted_lines {
        let Some(path_histories) = histories.get(&deleted.path) else {
            continue;
        };
        let Some(blame) = &deleted.blame else {
            continue;
        };
        let Some(history) = path_histories.get(&blame.oid) else {
            continue;
        };
        let candidate_references = message_references_from_parts(&history.subject, &history.body);
        let shared_reference = references
            .iter()
            .find(|reference| {
                candidate_references
                    .iter()
                    .any(|candidate| candidate == *reference)
            })
            .cloned();
        let failure = path_histories
            .values()
            .filter(|candidate| {
                pre_fix.contains(&candidate.oid)
                    && candidate.position > history.position
                    && candidate.oid != *fix_oid
                    && candidate.oid != blame.oid
            })
            .filter_map(|candidate| {
                let candidate_references =
                    message_references_from_parts(&candidate.subject, &candidate.body);
                let reference = references.iter().find(|reference| {
                    candidate_references
                        .iter()
                        .any(|candidate| candidate == *reference)
                })?;
                Some((
                    candidate.position,
                    candidate.oid.clone(),
                    candidate.subject.clone(),
                    reference,
                ))
            })
            .max_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)))
            .map(|(_, oid, subject, _)| (oid, subject));
        let movement = path_histories
            .values()
            .filter(|candidate| candidate.oid != blame.oid && anchored_moved(candidate))
            .filter(|candidate| pre_fix.is_empty() || pre_fix.contains(&candidate.oid))
            .max_by_key(|candidate| candidate.position)
            .map(|candidate| (candidate.oid.clone(), candidate.subject.clone()));
        let introducing_paths = anchored_paths(history);
        let mut paths = introducing_paths.clone();
        if let Some((movement_oid, _)) = &movement
            && let Some(movement_history) = path_histories.get(movement_oid)
        {
            for path in anchored_paths(movement_history) {
                if !paths.iter().any(|existing| existing == &path) {
                    paths.push(path);
                }
            }
        }
        let moved = anchored_moved(history) || movement.is_some();
        let evidence = by_oid.entry(blame.oid.clone()).or_insert_with(|| Evidence {
            oid: blame.oid.clone(),
            subject: history.subject.clone(),
            commit_time: history.commit_time,
            parent_count: history.parent_count,
            boundary: shallow && blame.boundary,
            moved,
            shared_reference: shared_reference.clone(),
            failure: failure.clone(),
            movement: movement.clone(),
            paths,
            patch_anchors: Vec::new(),
            lines: Vec::new(),
        });
        if let Some(reference) = shared_reference {
            evidence.shared_reference = Some(reference);
        }
        if !evidence.paths.iter().any(|path| path == &deleted.path) {
            evidence.paths.push(deleted.path.clone());
        }
        evidence.lines.push(deleted.line);
        evidence.patch_anchors.push(TraceFixPatchAnchor {
            line: blame.original_line,
            paths: introducing_paths,
        });
        evidence.boundary |= shallow && blame.boundary;
    }
    by_oid.into_values().collect()
}

fn anchored_moved(history: &retrieval::HistoryCommit) -> bool {
    history.changes.iter().any(|change| {
        history.anchored_ordinals.contains(&change.ordinal)
            && (change.status.starts_with('R') || change.status.starts_with('C'))
    })
}

fn anchored_paths(history: &retrieval::HistoryCommit) -> Vec<Vec<u8>> {
    let mut paths = Vec::new();
    for change in &history.changes {
        if !history.anchored_ordinals.contains(&change.ordinal) {
            continue;
        }
        for path in [&change.old_path, &change.new_path].into_iter().flatten() {
            if !paths.iter().any(|existing| existing == path) {
                paths.push(path.clone());
            }
        }
    }
    paths
}

fn message_references(message: &[u8]) -> Vec<String> {
    let (subject, body) = retrieval::message_parts(message);
    message_references_from_parts(&subject, &body)
}

fn message_references_from_parts(subject: &str, body: &str) -> Vec<String> {
    let mut references = subject
        .split_whitespace()
        .chain(body.split_whitespace())
        .filter_map(normalize_reference)
        .collect::<Vec<_>>();
    references.sort();
    references.dedup();
    references
}

fn normalize_reference(word: &str) -> Option<String> {
    let trimmed = word.trim_matches(|character: char| {
        !character.is_ascii_alphanumeric() && character != '#' && character != '-'
    });
    let number = trimmed.strip_prefix('#')?;
    (!number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| trimmed.to_owned())
}
