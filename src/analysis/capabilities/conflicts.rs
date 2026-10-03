//! Direct cached file-change evidence, separately oriented to pinned merge sides.
use crate::analysis::query::{Outcome, QueryReport};
use crate::{app::AppError, cache, git};
use serde::Serialize;

const FILE_LIMIT: usize = 100;
const REGION_LIMIT: usize = 20;
const MESSAGE_LIMIT: usize = 1000;

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
}
#[derive(Serialize)]
pub(crate) struct File {
    pub(crate) path: String,
    pub(crate) path_bytes: Vec<u8>,
    pub(crate) unsupported: Option<String>,
    pub(crate) sides: Vec<Side>,
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
                    let hunks = session.history_hunks(&commit.oid)?;
                    let matching: Vec<_> = hunks
                        .into_iter()
                        .filter(|hunk| {
                            commit.changes.iter().any(|change| {
                                change.ordinal == hunk.change_ordinal
                                    && (change.old_path.as_deref() == Some(file.path.as_slice())
                                        || change.new_path.as_deref() == Some(file.path.as_slice()))
                            })
                        })
                        .collect();
                    if matching.is_empty() {
                        continue;
                    }
                    let regions_truncated = matching.len() > REGION_LIMIT;
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
        files.push(File {
            path: String::from_utf8_lossy(&file.path).into_owned(),
            path_bytes: file.path,
            unsupported: file.unsupported,
            sides,
        });
    }
    let coverage_complete = warnings.is_empty()
        && !files_truncated
        && files.iter().all(|file| file.unsupported.is_none());
    let report = Report {
        schema_version: 1, ours: target.ours, theirs: target.theirs, merge_base: target.base,
        total_unmerged_paths, selected_paths, files, files_truncated, coverage_complete,
        limits: Limits { files: FILE_LIMIT, leads_per_file_side: limit, regions_per_lead: REGION_LIMIT, message_characters: MESSAGE_LIMIT },
        limitations: vec!["Direct same-path textual changes since the merge base only; no rename lineage, older history, associated changes, semantic claims or generated resolution. Cached merge changes are first-parent diffs.".to_owned(), "Commit message bodies are recorded participant reasons, bounded and decoded as UTF-8 with replacement for invalid bytes (message_lossy identifies affected leads); absence does not establish that no reason existed.".to_owned()],
        warnings,
    };
    Ok(Outcome {
        progress,
        warnings: Vec::new(),
        report: QueryReport::Conflicts(report),
    })
}
