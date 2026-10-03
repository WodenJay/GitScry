//! Direct cached file-change evidence, separately oriented to pinned merge sides.
use crate::analysis::{
    provenance::revert_reason,
    query::{self, Outcome, QueryReport},
    retrieval,
};
use crate::{
    app::AppError,
    cache::{self, QuerySession, SearchFilter},
    git,
};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
const FILE_LIMIT: usize = 100;
const REGION_LIMIT: usize = 20;
const MESSAGE_LIMIT: usize = 1000;
const RELATED_HISTORY_LIMIT: usize = 3;

#[derive(Serialize)]
pub(crate) struct Report {
    pub(crate) schema_version: u8,
    pub(crate) ours: String,
    pub(crate) theirs: String,
    pub(crate) merge_base: String,
    pub(crate) total_unmerged_paths: usize,
    pub(crate) selected_paths: usize,
    pub(crate) files: Vec<File>,
    pub(crate) files_truncated: bool,
    pub(crate) coverage_complete: bool,
    pub(crate) limits: Limits,
    pub(crate) limitations: Vec<String>,
    pub(crate) warnings: Vec<String>,
}
#[derive(Serialize)]
pub(crate) struct Limits {
    files: usize,
    leads_per_file_side: usize,
    regions_per_lead: usize,
    message_characters: usize,
    related_history_per_file: usize,
}
#[derive(Serialize)]
pub(crate) struct File {
    pub(crate) path: String,
    pub(crate) path_bytes: Vec<u8>,
    pub(crate) unsupported: Option<String>,
    pub(crate) sides: Vec<Side>,
    pub(crate) related_history: Vec<RelatedLead>,
    pub(crate) related_history_total: usize,
    pub(crate) related_history_truncated: bool,
}
#[derive(Serialize)]
pub(crate) struct Side {
    pub(crate) name: &'static str,
    pub(crate) endpoint: String,
    pub(crate) leads: Vec<Lead>,
    pub(crate) total_leads: usize,
    pub(crate) truncated: bool,
}
#[derive(Serialize)]
pub(crate) struct Lead {
    pub(crate) commit: String,
    pub(crate) path: String,
    pub(crate) association: &'static str,
    pub(crate) subject: String,
    pub(crate) recorded_reason: Option<String>,
    pub(crate) reason_source: &'static str,
    pub(crate) message_truncated: bool,
    pub(crate) message_lossy: bool,
    pub(crate) regions: Vec<Region>,
    pub(crate) regions_truncated: bool,
}
#[derive(Serialize)]
pub(crate) struct RelatedLead {
    pub(crate) commit: String,
    pub(crate) path: String,
    pub(crate) association: &'static str,
    pub(crate) related_sides: [&'static str; 2],
    pub(crate) selection_basis: Vec<&'static str>,
    pub(crate) subject: String,
    pub(crate) recorded_reason: Option<String>,
    pub(crate) reason_source: &'static str,
    pub(crate) reverted_by: Option<HistoricalReference>,
    pub(crate) reverts_commit: Option<String>,
    pub(crate) message_truncated: bool,
    pub(crate) message_lossy: bool,
    pub(crate) regions: Vec<Region>,
    pub(crate) regions_truncated: bool,
}
#[derive(Serialize)]
pub(crate) struct HistoricalReference {
    pub(crate) commit: String,
    pub(crate) subject: String,
}
#[derive(Serialize)]
pub(crate) struct Region {
    pub(crate) change_ordinal: i64,
    pub(crate) hunk_ordinal: i64,
    pub(crate) old_start: i64,
    pub(crate) old_lines: i64,
    pub(crate) new_start: i64,
    pub(crate) new_lines: i64,
}
fn bounded(text: &str) -> (String, bool) {
    (
        text.chars().take(MESSAGE_LIMIT).collect(),
        text.chars().count() > MESSAGE_LIMIT,
    )
}

pub(crate) fn execute(paths: Vec<String>, limit: usize) -> Result<Outcome, AppError> {
    let repository = git::Repository::discover()?;
    let mut target = repository.merge_conflict()?;
    let total_unmerged_paths = target.files.len();
    let paths: Vec<_> = paths.iter().map(|path| path.replace('\\', "/")).collect();
    for path in &paths {
        if !target.files.iter().any(|file| file.path == path.as_bytes()) {
            return Err(AppError::input(format!(
                "path {path:?} is not an unmerged path; use exact repository-relative paths"
            )));
        }
    }
    if !paths.is_empty() {
        target
            .files
            .retain(|file| paths.iter().any(|path| file.path == path.as_bytes()));
    }
    let selected_paths = target.files.len();
    let files_truncated = selected_paths > FILE_LIMIT;
    target.files.truncate(FILE_LIMIT);
    let mut progress = Vec::new();
    for endpoint in [&target.ours, &target.theirs] {
        let prepared = cache::prepare_at(&repository, endpoint.clone(), &mut |_| {})?;
        progress.extend(prepared.release().0);
    }
    let session = cache::open_query(&repository)?;
    for oid in [&target.ours, &target.theirs, &target.base] {
        session.require_revision(oid)?;
    }
    progress.extend_from_slice(session.progress());
    let mut warnings = session.warnings().to_vec();
    warnings.extend(
        progress
            .iter()
            .filter(|line| line.starts_with("warning:"))
            .cloned(),
    );
    let cached = session.commit_oids()?;
    let excluded = session.ancestors(&target.base)?;
    let mut histories = Vec::new();
    for (name, endpoint) in [("ours", &target.ours), ("theirs", &target.theirs)] {
        let mut reachable = session.ancestors(endpoint)?;
        let missing = reachable.difference(&cached).count();
        if missing > 0 {
            warnings.push(format!("{name} endpoint {endpoint}: incomplete history coverage; {missing} known ancestor(s) unavailable in the prepared cache (for example, beyond a shallow boundary)"));
        }
        reachable.retain(|oid| cached.contains(oid) && !excluded.contains(oid));
        histories.push((name, endpoint, reachable));
    }

    let shared_reachable = excluded
        .intersection(&cached)
        .cloned()
        .collect::<HashSet<_>>();
    let shared_revisions = shared_reachable.iter().cloned().collect::<Vec<_>>();
    let shared_scope =
        query::scope::install_shared_history_scope(&session, &shared_revisions, &target.base)?;
    let reverts = retrieval::reverts(&session, Some(&shared_scope))?;
    warnings.sort();
    warnings.dedup();
    let mut files = Vec::new();
    for file in target.files {
        let mut sides = Vec::new();
        if file.unsupported.is_none() {
            for (name, endpoint, reachable) in &histories {
                let history = session.path_history(&file.path, reachable)?;
                let mut leads = Vec::new();
                for commit in history {
                    let (regions, regions_truncated) =
                        history_regions(&session, &commit, &file.path)?;
                    if regions.is_empty() {
                        continue;
                    }
                    let (subject, subject_truncated) = bounded(&commit.subject);
                    let (reason, reason_truncated) = bounded(&commit.body);
                    let message_lossy = session
                        .commit_message(&commit.oid)?
                        .is_some_and(|message| std::str::from_utf8(&message).is_err());
                    leads.push(Lead {
                        commit: commit.oid, path: String::from_utf8_lossy(&file.path).into_owned(),
                        association: "same-path textual change after merge base (containing-file evolution; not conflict-region lineage)",
                        subject, recorded_reason: (!reason.trim().is_empty()).then_some(reason),
                        reason_source: "commit message body (recorded participant text; not inferred or verified)",
                        message_truncated: subject_truncated || reason_truncated,
                        message_lossy,
                        regions, regions_truncated,
                    });
                }
                let total_leads = leads.len();
                leads.truncate(limit);
                sides.push(Side {
                    name,
                    endpoint: (*endpoint).clone(),
                    leads,
                    total_leads,
                    truncated: total_leads > limit,
                });
            }
        }

        let (related_history, related_history_total, related_history_truncated) =
            if file.unsupported.is_none() {
                related_history(
                    &session,
                    &file.path,
                    &shared_reachable,
                    &shared_scope,
                    &reverts,
                    RELATED_HISTORY_LIMIT,
                )?
            } else {
                (Vec::new(), 0, false)
            };
        files.push(File {
            path: String::from_utf8_lossy(&file.path).into_owned(),
            path_bytes: file.path,
            unsupported: file.unsupported,
            sides,
            related_history,
            related_history_total,
            related_history_truncated,
        });
    }
    let coverage_complete = warnings.is_empty()
        && !files_truncated
        && files.iter().all(|file| file.unsupported.is_none());
    let report = Report {
        schema_version: 1, ours: target.ours, theirs: target.theirs, merge_base: target.base,
        total_unmerged_paths, selected_paths, files, files_truncated, coverage_complete,
        limits: Limits { files: FILE_LIMIT, leads_per_file_side: limit, regions_per_lead: REGION_LIMIT, message_characters: MESSAGE_LIMIT, related_history_per_file: RELATED_HISTORY_LIMIT },
        limitations: vec![
            "Direct side leads are same-path textual changes after the merge base. Related history is limited to same-path changes in cached merge-base ancestry, shared by both endpoints; these are contextual precedents, not causal explanations. No rename lineage, semantic claims or generated resolution. Cached merge changes are first-parent diffs.".to_owned(),
            "Commit message bodies and revert reasons are recorded participant text, not inferred or verified; absent reasons remain unknown. Messages are bounded and decoded as UTF-8 with replacement for invalid bytes (message_lossy identifies affected leads).".to_owned(),
            "Related history is capped at three leads per conflicted file; related_history_truncated marks output truncation.".to_owned(),
        ],
        warnings,
    };
    Ok(Outcome {
        progress,
        warnings: Vec::new(),
        report: QueryReport::Conflicts(report),
    })
}

fn related_history(
    session: &QuerySession,
    path: &[u8],
    reachable: &HashSet<String>,
    scope: &SearchFilter,
    reverts: &retrieval::RevertIndex,
    limit: usize,
) -> Result<(Vec<RelatedLead>, usize, bool), AppError> {
    let history = session
        .path_history(path, reachable)?
        .into_iter()
        .filter(|commit| {
            commit.changes.iter().any(|change| {
                change.old_path.as_deref() == Some(path) || change.new_path.as_deref() == Some(path)
            })
        })
        .collect::<Vec<_>>();
    let history_oids = history
        .iter()
        .map(|commit| commit.oid.clone())
        .collect::<HashSet<_>>();
    let mut paired_reverts = HashSet::new();
    let mut linked_reverts = HashMap::new();
    for commit in &history {
        if let Some(revert) = retrieval::link_change(
            session,
            reverts,
            Some(scope),
            &commit.oid,
            &commit.paths,
            commit.position,
        )? {
            if history_oids.contains(&revert.oid) {
                paired_reverts.insert(revert.oid.clone());
            }
            linked_reverts.insert(commit.oid.clone(), revert);
        }
    }

    let total = history.len().saturating_sub(paired_reverts.len());
    let mut candidates = history
        .into_iter()
        .filter(|commit| !paired_reverts.contains(&commit.oid))
        .collect::<Vec<_>>();
    // Put explicit revert links first; keep recency within each priority.
    candidates.sort_by_key(|commit| {
        !(linked_reverts.contains_key(&commit.oid) || reverts.is_revert(&commit.oid))
    });
    let mut leads = Vec::new();
    for commit in candidates.into_iter().take(limit) {
        let linked_revert = linked_reverts.get(&commit.oid).copied();
        let is_revert = reverts.is_revert(&commit.oid);
        let (subject, subject_truncated) = bounded(&commit.subject);
        let (recorded_reason, reason_truncated, reason_source) = if let Some(revert) = linked_revert
        {
            let (reason, truncated) =
                bounded_optional(revert_reason(&revert.subject, &revert.body));
            (
                reason,
                truncated,
                "recorded in the reverting commit body; not inferred",
            )
        } else if is_revert {
            let (reason, truncated) =
                bounded_optional(revert_reason(&commit.subject, &commit.body));
            (
                reason,
                truncated,
                "recorded in this revert commit body; not inferred",
            )
        } else {
            let (reason, truncated) = bounded(&commit.body);
            (
                (!reason.trim().is_empty()).then_some(reason),
                truncated,
                "commit message body (recorded participant text; not inferred or verified)",
            )
        };
        let (reverted_by, revert_subject_truncated, revert_message_lossy) =
            if let Some(revert) = linked_revert {
                let (subject, truncated) = bounded(&revert.subject);
                (
                    Some(HistoricalReference {
                        commit: revert.oid.clone(),
                        subject,
                    }),
                    truncated,
                    message_is_lossy(session, &revert.oid)?,
                )
            } else {
                (None, false, false)
            };
        let mut selection_basis = vec![
            "same conflicted path changed in cached history reachable from the merge base",
            "merge-base ancestry is shared by the pinned ours and theirs endpoints",
        ];
        if linked_revert.is_some() {
            selection_basis.push(if reverts.of(&commit.oid).is_some() {
                "revert trailer names this change"
            } else {
                "revert covers all changed paths without an intervening path touch"
            });
        } else if is_revert {
            selection_basis.push("commit subject identifies a revert or rollback");
        }
        let (regions, regions_truncated) = history_regions(session, &commit, path)?;
        leads.push(RelatedLead {
            commit: commit.oid.clone(),
            path: String::from_utf8_lossy(path).into_owned(),
            association: "shared pre-merge-base path history",
            related_sides: ["ours", "theirs"],
            selection_basis,
            subject,
            recorded_reason,
            reason_source,
            reverted_by,
            reverts_commit: reverts.target_of(&commit.oid).map(str::to_owned),
            message_truncated: subject_truncated || reason_truncated || revert_subject_truncated,
            message_lossy: message_is_lossy(session, &commit.oid)? || revert_message_lossy,
            regions,
            regions_truncated,
        });
    }
    let truncated = total > leads.len();
    Ok((leads, total, truncated))
}

fn history_regions(
    session: &QuerySession,
    commit: &cache::HistoryCommit,
    path: &[u8],
) -> Result<(Vec<Region>, bool), AppError> {
    let matching = session
        .history_hunks(&commit.oid)?
        .into_iter()
        .filter(|hunk| {
            commit.changes.iter().any(|change| {
                change.ordinal == hunk.change_ordinal
                    && (change.old_path.as_deref() == Some(path)
                        || change.new_path.as_deref() == Some(path))
            })
        })
        .collect::<Vec<_>>();
    let truncated = matching.len() > REGION_LIMIT;
    let regions = matching
        .into_iter()
        .take(REGION_LIMIT)
        .map(|hunk| Region {
            change_ordinal: hunk.change_ordinal,
            hunk_ordinal: hunk.hunk_ordinal,
            old_start: hunk.old_start,
            old_lines: hunk.old_lines,
            new_start: hunk.new_start,
            new_lines: hunk.new_lines,
        })
        .collect();
    Ok((regions, truncated))
}

fn bounded_optional(text: Option<String>) -> (Option<String>, bool) {
    text.map_or((None, false), |text| {
        let (bounded, truncated) = bounded(&text);
        (Some(bounded), truncated)
    })
}

fn message_is_lossy(session: &QuerySession, oid: &str) -> Result<bool, AppError> {
    Ok(session
        .commit_message(oid)?
        .is_some_and(|message| std::str::from_utf8(&message).is_err()))
}
