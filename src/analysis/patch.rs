use std::collections::{HashMap, HashSet};

use super::{CodeDirection, Detail, Intent, Material, Report, anchors_overlap};
use crate::{
    app::AppError,
    cache::{HunkId, PatchHistoryHunk, QuerySession},
};

const MAX_SCANNED_HUNKS: usize = 64;
const MAX_EXCERPTS: usize = 16;
const MAX_EXCERPT_FILES: usize = 8;
const MAX_CACHED_HUNK_BYTES: usize = 64 * 1024;
const MAX_EXCERPT_BYTES: usize = 8 * 1024;
const TRACE_CONTEXT_LINES: usize = 3;
const MAX_RESULT_EXCERPT_BYTES: usize = 32 * 1024;

pub(crate) struct PatchExcerpt {
    pub(crate) commit_oid: String,
    pub(crate) status: PatchStatus,
    pub(crate) hunks: Vec<PatchHunk>,
    pub(crate) truncated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PatchStatus {
    Available,
    NoRelevantHunks,
    NoRelevantHunk,
    Unavailable,
}

impl PatchStatus {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::NoRelevantHunks => "no_relevant_hunks",
            Self::NoRelevantHunk => "no_relevant_hunk",
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
    attach_patch_excerpts_using(session, report, |_, _, cached| {
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

pub(crate) fn attach_trace_fix_patch_excerpts(
    session: &QuerySession,
    report: &mut Report,
) -> Result<(), AppError> {
    attach_patch_excerpts_using(session, report, |material, _, cached| {
        let Some(Detail::TraceFix(trace)) = material.detail.as_ref() else {
            return None;
        };
        let relevant = trace.patch_anchors.iter().any(|anchor| {
            cached
                .new_path
                .as_ref()
                .is_some_and(|new_path| anchor.paths.iter().any(|path| path == new_path))
                && hunk_contains_line(cached, anchor.line)
        });
        relevant.then_some(0)
    })
}

pub(crate) fn attach_selected_patch_excerpts(
    session: &QuerySession,
    report: &mut Report,
    priorities: &HunkPriorities,
) -> Result<(), AppError> {
    attach_patch_excerpts_using(session, report, |_, oid, cached| {
        priorities
            .get(oid)
            .and_then(|hunks| hunks.get(&cached.id()).copied())
    })
}

pub(crate) fn attach_why_patch_excerpts(
    session: &QuerySession,
    report: &mut Report,
    priorities: &HunkPriorities,
) -> Result<(), AppError> {
    report.patch_mode = true;
    let Some(why) = report.why.as_mut() else {
        return Ok(());
    };
    for modification in &mut why.target_related_modifications {
        modification.patch = Some(selected_patch_excerpt(
            session,
            &modification.oid,
            |cached| {
                priorities
                    .get(&modification.oid)
                    .and_then(|hunks| hunks.get(&cached.id()).copied())
            },
        )?);
    }
    if let super::WhyAttribution::Available(attribution) = &mut why.attribution {
        attribution.patch = Some(selected_patch_excerpt(
            session,
            &attribution.oid,
            |cached| {
                priorities
                    .get(&attribution.oid)
                    .and_then(|hunks| hunks.get(&cached.id()).copied())
            },
        )?);
    }
    Ok(())
}

fn attach_patch_excerpts_using(
    session: &QuerySession,
    report: &mut Report,
    mut priority_for: impl FnMut(&Material, &str, &PatchHistoryHunk) -> Option<usize>,
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
        let oid = citation.oid.clone();
        let patch =
            selected_patch_excerpt(session, &oid, |cached| priority_for(material, &oid, cached))?;
        material.patch = Some(patch);
    }
    Ok(())
}

pub(crate) fn selected_patch_excerpt(
    session: &QuerySession,
    oid: &str,
    mut priority_for: impl FnMut(&PatchHistoryHunk) -> Option<usize>,
) -> Result<PatchExcerpt, AppError> {
    let history = session.patch_history(oid, MAX_SCANNED_HUNKS, MAX_CACHED_HUNK_BYTES)?;
    let has_cached_hunks = !history.hunks.is_empty();
    let mut truncated = history.truncated;
    let mut hunks = Vec::new();
    let mut remaining_bytes = MAX_RESULT_EXCERPT_BYTES;
    let mut excerpt_paths = HashSet::new();
    let mut selected = history
        .hunks
        .into_iter()
        .filter_map(|cached| priority_for(&cached).map(|priority| (priority, cached)))
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
        if excerpt_paths.len() + cached_paths.difference(&excerpt_paths).count() > MAX_EXCERPT_FILES
        {
            truncated = true;
            break;
        }
        excerpt_paths.extend(cached_paths);
        if remaining_bytes == 0 {
            truncated = true;
            break;
        }
        let excerpt = bounded_patch_hunk(cached, &mut remaining_bytes);
        truncated |= excerpt.truncated;
        hunks.push(excerpt);
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
    Ok(PatchExcerpt {
        commit_oid: oid.to_owned(),
        status,
        hunks,
        truncated,
    })
}

pub(crate) fn attach_timeline_patch_excerpts(
    session: &QuerySession,
    report: &mut super::TimelineReport,
) -> Result<(), AppError> {
    report.patch_mode = true;
    for entry in &mut report.entries {
        // Keep availability scoped to this change so sibling hunks cannot make
        // binary or metadata-only entries appear to have an attributable patch.
        let history = session.patch_history_for_change(
            &entry.commit_id,
            entry.change_ordinal,
            MAX_SCANNED_HUNKS,
            MAX_CACHED_HUNK_BYTES,
        )?;
        let has_change_hunks = !history.hunks.is_empty();
        let mut truncated = history.truncated;
        let mut remaining_bytes = MAX_RESULT_EXCERPT_BYTES;
        let mut hunks = Vec::new();

        for cached in history.hunks {
            let is_entry_path = [&cached.old_path, &cached.new_path]
                .into_iter()
                .flatten()
                .any(|path| path.as_slice() == entry.path.as_slice());
            if !is_entry_path {
                continue;
            }
            if hunks.len() == MAX_EXCERPTS || remaining_bytes == 0 {
                truncated = true;
                break;
            }

            let excerpt = bounded_patch_hunk(cached, &mut remaining_bytes);
            truncated |= excerpt.truncated;
            hunks.push(excerpt);
        }

        let status = timeline_patch_status(
            has_change_hunks,
            !hunks.is_empty(),
            history.truncated || history.missing_objects,
        );
        entry.patch = Some(PatchExcerpt {
            commit_oid: entry.commit_id.clone(),
            status,
            hunks,
            truncated,
        });
    }
    Ok(())
}

pub(crate) fn attach_trace_removal_patch_excerpts(
    session: &QuerySession,
    report: &mut super::TraceRemovalReport,
) -> Result<(), AppError> {
    for event in &mut report.events {
        let matching_lines = event
            .matches
            .iter()
            .filter(|matched| {
                matched.direction == CodeDirection::Removed
                    && matched.path.as_slice() == event.old_path.as_slice()
            })
            .filter_map(|matched| i64::try_from(matched.line_number).ok())
            .collect::<HashSet<_>>();
        let history = session.patch_history_for_change_at_lines(
            &event.commit_id,
            event.change_ordinal,
            &matching_lines,
            MAX_SCANNED_HUNKS,
            MAX_CACHED_HUNK_BYTES,
        )?;
        let mut truncated = history.truncated;
        let mut remaining_bytes = MAX_RESULT_EXCERPT_BYTES;
        let mut hunks = Vec::new();
        'cached: for cached in history.hunks {
            if cached.old_path.as_deref() != Some(event.old_path.as_slice()) {
                continue;
            }
            let Some(text) = cached.text.as_deref() else {
                continue;
            };
            for fragment in trace_removal_fragments(&cached, text, &matching_lines) {
                if hunks.len() == MAX_EXCERPTS || remaining_bytes == 0 {
                    truncated = true;
                    break 'cached;
                }
                let excerpt = bounded_patch_hunk(fragment, &mut remaining_bytes);
                truncated |= excerpt.truncated;
                hunks.push(excerpt);
            }
        }
        event.patch = Some(PatchExcerpt {
            commit_oid: event.commit_id.clone(),
            status: if hunks.is_empty() {
                PatchStatus::Unavailable
            } else {
                PatchStatus::Available
            },
            hunks,
            truncated,
        });
    }
    Ok(())
}

struct PatchDiffRow {
    text: Vec<u8>,
    old_before: i64,
    new_before: i64,
    old_line: Option<i64>,
    consumes_old: bool,
    consumes_new: bool,
}

fn trace_removal_fragments(
    cached: &PatchHistoryHunk,
    text: &[u8],
    matching_lines: &HashSet<i64>,
) -> Vec<PatchHistoryHunk> {
    let mut old_cursor = cached.old_start;
    let mut new_cursor = cached.new_start;
    let mut rows = Vec::new();
    for raw in text.split_inclusive(|byte| *byte == b'\n') {
        let line = raw.strip_suffix(b"\n").unwrap_or(raw);
        let Some(&kind) = line.first() else {
            continue;
        };
        if line.starts_with(b"@@") {
            continue;
        }
        let (consumes_old, consumes_new) = match kind {
            b'-' => (true, false),
            b'+' => (false, true),
            b' ' => (true, true),
            b'\\' => (false, false),
            _ => continue,
        };
        rows.push(PatchDiffRow {
            text: raw.to_vec(),
            old_before: old_cursor,
            new_before: new_cursor,
            old_line: (kind == b'-').then_some(old_cursor),
            consumes_old,
            consumes_new,
        });
        if consumes_old {
            old_cursor = old_cursor.saturating_add(1);
        }
        if consumes_new {
            new_cursor = new_cursor.saturating_add(1);
        }
    }
    if rows.is_empty() {
        return Vec::new();
    }
    let matching_rows = rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| {
            row.old_line
                .filter(|line| matching_lines.contains(line))
                .map(|_| index)
        })
        .collect::<Vec<_>>();
    let mut windows: Vec<(usize, usize, Vec<usize>)> = Vec::new();
    for &matched in &matching_rows {
        let start = matched.saturating_sub(TRACE_CONTEXT_LINES);
        let end = matched
            .saturating_add(TRACE_CONTEXT_LINES)
            .min(rows.len() - 1);
        if let Some((_, previous_end, previous_matches)) = windows.last_mut()
            && start <= previous_end.saturating_add(1)
        {
            *previous_end = end;
            previous_matches.push(matched);
            continue;
        }
        windows.push((start, end, vec![matched]));
    }

    let mut fragments = Vec::new();
    for (start, end, anchors) in windows {
        if let Some((start, end)) = fit_trace_window(&rows, &anchors, start, end) {
            fragments.push(trace_patch_hunk(cached, &rows, start, end));
            continue;
        }
        for (index, &anchor) in anchors.iter().enumerate() {
            let mut start = anchor.saturating_sub(TRACE_CONTEXT_LINES);
            let mut end = anchor
                .saturating_add(TRACE_CONTEXT_LINES)
                .min(rows.len() - 1);
            if let Some(&previous) = index.checked_sub(1).and_then(|i| anchors.get(i)) {
                start = start.max(previous + (anchor - previous) / 2 + 1);
            }
            if let Some(&next) = anchors.get(index + 1) {
                end = end.min(anchor + (next - anchor) / 2);
            }
            let range = fit_trace_window(&rows, &[anchor], start, end).unwrap_or((anchor, anchor));
            fragments.push(trace_patch_hunk(cached, &rows, range.0, range.1));
        }
    }
    fragments
}

fn fit_trace_window(
    rows: &[PatchDiffRow],
    anchors: &[usize],
    mut start: usize,
    mut end: usize,
) -> Option<(usize, usize)> {
    let first_anchor = *anchors.first()?;
    let last_anchor = *anchors.last()?;
    while trace_fragment_size(rows, start, end) > MAX_EXCERPT_BYTES {
        let can_trim_start = start < first_anchor;
        let can_trim_end = end > last_anchor;
        if !can_trim_start && !can_trim_end {
            return None;
        }
        let trim_start =
            can_trim_start && (!can_trim_end || first_anchor - start >= end - last_anchor);
        if trim_start {
            start += 1;
        } else {
            end -= 1;
        }
    }
    Some((start, end))
}

fn trace_fragment_size(rows: &[PatchDiffRow], start: usize, end: usize) -> usize {
    let header_len = trace_fragment_metadata(rows, start, end);
    let header = format!(
        "@@ -{},{} +{},{} @@\n",
        header_len.0, header_len.1, header_len.2, header_len.3
    );
    rows[start..=end].iter().fold(header.len(), |size, row| {
        size.saturating_add(row.text.len())
    })
}

fn trace_fragment_metadata(
    rows: &[PatchDiffRow],
    start: usize,
    end: usize,
) -> (i64, i64, i64, i64) {
    let first = &rows[start];
    let old_start = if first.consumes_old {
        first.old_before
    } else {
        first.old_before.saturating_sub(1)
    };
    let new_start = if first.consumes_new {
        first.new_before
    } else {
        first.new_before.saturating_sub(1)
    };
    let old_lines = rows[start..=end]
        .iter()
        .filter(|row| row.consumes_old)
        .count() as i64;
    let new_lines = rows[start..=end]
        .iter()
        .filter(|row| row.consumes_new)
        .count() as i64;
    (old_start, old_lines, new_start, new_lines)
}

fn trace_patch_hunk(
    cached: &PatchHistoryHunk,
    rows: &[PatchDiffRow],
    start: usize,
    end: usize,
) -> PatchHistoryHunk {
    let (old_start, old_lines, new_start, new_lines) = trace_fragment_metadata(rows, start, end);
    let mut text =
        format!("@@ -{old_start},{old_lines} +{new_start},{new_lines} @@\n").into_bytes();
    for row in &rows[start..=end] {
        text.extend_from_slice(&row.text);
    }
    PatchHistoryHunk {
        change_ordinal: cached.change_ordinal,
        old_path: cached.old_path.clone(),
        new_path: cached.new_path.clone(),
        old_start,
        old_lines,
        new_start,
        new_lines,
        hunk_ordinal: cached.hunk_ordinal,
        text: Some(text),
    }
}

fn bounded_patch_hunk(cached: PatchHistoryHunk, remaining_bytes: &mut usize) -> PatchHunk {
    let (text, truncated) = match cached.text {
        Some(mut text) => {
            let allowed = MAX_EXCERPT_BYTES.min(*remaining_bytes);
            let clipped = text.len() > allowed;
            text.truncate(allowed);
            (Some(text), clipped)
        }
        None => (None, true),
    };
    if let Some(text) = &text {
        *remaining_bytes -= text.len();
    }
    PatchHunk {
        old_path: cached.old_path,
        new_path: cached.new_path,
        old_start: cached.old_start,
        old_lines: cached.old_lines,
        new_start: cached.new_start,
        new_lines: cached.new_lines,
        text,
        truncated,
    }
}

fn timeline_patch_status(
    has_change_hunks: bool,
    has_attributable_hunks: bool,
    history_incomplete: bool,
) -> PatchStatus {
    if !has_change_hunks || (!has_attributable_hunks && history_incomplete) {
        PatchStatus::Unavailable
    } else if has_attributable_hunks {
        PatchStatus::Available
    } else {
        PatchStatus::NoRelevantHunk
    }
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

fn hunk_contains_line(hunk: &PatchHistoryHunk, line: usize) -> bool {
    let Ok(line) = i64::try_from(line) else {
        return false;
    };
    if hunk.new_lines <= 0 {
        return false;
    }
    let Some(end) = hunk.new_start.checked_add(hunk.new_lines) else {
        return false;
    };
    line >= hunk.new_start && line < end
}

fn hunk_matches_terms(text: &[u8], intent: &Intent) -> bool {
    let text = String::from_utf8_lossy(text);
    let terms = super::retrieval::tokenize(&text);
    intent.terms().iter().any(|term| terms.contains(term))
}

#[cfg(test)]
mod tests {
    use super::{PatchStatus, timeline_patch_status};

    #[test]
    fn timeline_patch_status_separates_availability_from_attribution() {
        assert_eq!(
            timeline_patch_status(false, false, false),
            PatchStatus::Unavailable
        );
        assert_eq!(
            timeline_patch_status(true, false, false),
            PatchStatus::NoRelevantHunk
        );
        assert_eq!(
            timeline_patch_status(true, false, true),
            PatchStatus::Unavailable
        );
        assert_eq!(
            timeline_patch_status(true, true, true),
            PatchStatus::Available
        );
    }
}
