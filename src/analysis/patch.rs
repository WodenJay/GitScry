use std::collections::{HashMap, HashSet};

use super::{Intent, Report, anchors_overlap};
use crate::{
    app::AppError,
    cache::{HunkId, PatchHistory, PatchHistoryHunk, QuerySession},
};

const MAX_SCANNED_HUNKS: usize = 64;
const MAX_EXCERPTS: usize = 16;
const MAX_EXCERPT_FILES: usize = 8;
const MAX_CACHED_HUNK_BYTES: usize = 64 * 1024;
const MAX_EXCERPT_BYTES: usize = 8 * 1024;
const MAX_RESULT_EXCERPT_BYTES: usize = 32 * 1024;

pub(crate) struct PatchExcerpt {
    pub(crate) commit_oid: String,
    pub(crate) status: PatchStatus,
    pub(crate) hunks: Vec<PatchHunk>,
    pub(crate) truncated: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum PatchStatus {
    Available,
    NoRelevantHunks,
    Unavailable,
}

impl PatchStatus {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::NoRelevantHunks => "no_relevant_hunks",
            Self::Unavailable => "unavailable",
        }
    }
}

pub(crate) struct PatchHunk {
    pub(crate) old_path: Option<Vec<u8>>,
    pub(crate) new_path: Option<Vec<u8>>,
    pub(crate) old_start: i64,
    pub(crate) old_lines: i64,
    pub(crate) new_start: i64,
    pub(crate) new_lines: i64,
    pub(crate) text: Option<Vec<u8>>,
    pub(crate) truncated: bool,
}

pub(crate) type HunkPriorities = HashMap<String, HashMap<HunkId, usize>>;

pub(crate) fn attach_patch_excerpts(
    session: &QuerySession,
    intent: &Intent,
    report: &mut Report,
    path_only: bool,
    path_filter: &[String],
) -> Result<(), AppError> {
    let path_anchors = path_filter
        .iter()
        .map(|path| super::normalize_path(path.as_bytes()))
        .collect::<Vec<_>>();
    attach_patch_excerpts_using(session, report, |_, cached| {
        let path_match = cached_path_matches(cached, intent, &path_anchors);
        let text_matches = cached
            .text
            .as_deref()
            .is_some_and(|text| hunk_matches_terms(text, intent));
        let relevant = if path_only {
            path_match && text_matches
        } else {
            path_match || text_matches
        };
        relevant.then_some(0)
    })
}

pub(crate) fn attach_selected_patch_excerpts(
    session: &QuerySession,
    report: &mut Report,
    priorities: &HunkPriorities,
) -> Result<(), AppError> {
    attach_patch_excerpts_using(session, report, |oid, cached| {
        priorities
            .get(oid)
            .and_then(|hunks| hunks.get(&cached.id()).copied())
    })
}

fn attach_patch_excerpts_using(
    session: &QuerySession,
    report: &mut Report,
    mut priority_for: impl FnMut(&str, &PatchHistoryHunk) -> Option<usize>,
) -> Result<(), AppError> {
    report.patch_mode = true;
    for material in &mut report.materials {
        let Some(citation) = material.citations.first() else {
            material.patch = Some(PatchExcerpt {
                commit_oid: String::new(),
                status: PatchStatus::Unavailable,
                hunks: Vec::new(),
                truncated: false,
            });
            continue;
        };

        let history: PatchHistory =
            session.patch_history(&citation.oid, MAX_SCANNED_HUNKS, MAX_CACHED_HUNK_BYTES)?;
        let has_cached_hunks = !history.hunks.is_empty();
        let mut truncated = history.truncated;
        let mut hunks = Vec::new();
        let mut remaining_bytes = MAX_RESULT_EXCERPT_BYTES;
        let mut excerpt_paths = HashSet::new();
        let mut selected = history
            .hunks
            .into_iter()
            .filter_map(|cached| {
                priority_for(&citation.oid, &cached).map(|priority| (priority, cached))
            })
            .collect::<Vec<_>>();
        selected.sort_by_key(|(priority, _)| *priority);

        for (_, cached) in selected {
            if hunks.len() == MAX_EXCERPTS {
                truncated = true;
                break;
            }
            let cached_paths = [&cached.old_path, &cached.new_path]
                .into_iter()
                .flatten()
                .cloned()
                .collect::<HashSet<_>>();
            if excerpt_paths.len() + cached_paths.difference(&excerpt_paths).count()
                > MAX_EXCERPT_FILES
            {
                truncated = true;
                break;
            }
            excerpt_paths.extend(cached_paths);
            if remaining_bytes == 0 {
                truncated = true;
                break;
            }

            let (text, hunk_truncated) = match cached.text {
                Some(mut text) => {
                    let allowed = MAX_EXCERPT_BYTES.min(remaining_bytes);
                    let clipped = text.len() > allowed;
                    text.truncate(allowed);
                    (Some(text), clipped)
                }
                None => (None, true),
            };
            if let Some(text) = &text {
                remaining_bytes -= text.len();
            }
            truncated |= hunk_truncated;
            hunks.push(PatchHunk {
                old_path: cached.old_path,
                new_path: cached.new_path,
                old_start: cached.old_start,
                old_lines: cached.old_lines,
                new_start: cached.new_start,
                new_lines: cached.new_lines,
                text,
                truncated: hunk_truncated,
            });
        }

        let status = if !has_cached_hunks
            || (hunks.is_empty() && (history.truncated || history.missing_objects))
        {
            PatchStatus::Unavailable
        } else if hunks.is_empty() {
            PatchStatus::NoRelevantHunks
        } else {
            PatchStatus::Available
        };
        material.patch = Some(PatchExcerpt {
            commit_oid: citation.oid.clone(),
            status,
            hunks,
            truncated,
        });
    }
    Ok(())
}

fn cached_path_matches(cached: &PatchHistoryHunk, intent: &Intent, path_filter: &[String]) -> bool {
    let paths = [&cached.old_path, &cached.new_path]
        .into_iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    let anchors = if path_filter.is_empty() {
        intent.anchors()
    } else {
        path_filter
    };
    anchors_overlap(&paths, anchors) > 0
}

fn hunk_matches_terms(text: &[u8], intent: &Intent) -> bool {
    let text = String::from_utf8_lossy(text);
    let terms = super::retrieval::tokenize(&text);
    intent.terms().iter().any(|term| terms.contains(term))
}
