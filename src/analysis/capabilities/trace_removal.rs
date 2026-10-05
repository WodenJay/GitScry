//! Text-matched deletion events, not inferred code lifecycles.
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet, HashSet},
    path::PathBuf,
};

use crate::analysis::retrieval::code_fragment::{self, Fragment, Span};
use crate::{
    analysis::{CodeDirection, CodeMatch, PatchExcerpt, SearchScopeInfo},
    app::AppError,
    cache::{CodeHunk, PatchHistoryHunk, QuerySession, SearchFilter},
};

use crate::analysis::patch::{
    MAX_CACHED_HUNK_BYTES, MAX_EXCERPT_BYTES, MAX_EXCERPTS, MAX_RESULT_EXCERPT_BYTES,
    MAX_SCANNED_HUNKS, PatchStatus, bounded_patch_hunk,
};

const TRACE_CONTEXT_LINES: usize = 3;
const MAX_SAME_COMMIT_FILE_CHANGES: usize = 12;

type SelectedEventGroups<T> = BTreeMap<(String, Vec<u8>), Vec<T>>;
#[derive(Clone, Copy)]
pub(crate) enum SameCommitFileStatus {
    Complete,
    Truncated,
    Unavailable,
}

impl SameCommitFileStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Truncated => "truncated",
            Self::Unavailable => "unavailable",
        }
    }

    pub(crate) fn is_complete(self) -> bool {
        matches!(self, Self::Complete)
    }
}

pub(crate) struct SameCommitFiles {
    pub(crate) status: SameCommitFileStatus,
    pub(crate) files: Vec<crate::cache::OtherFileChange>,
}

pub(crate) struct Report {
    pub(crate) query: String,
    pub(crate) path: Option<Vec<u8>>,
    pub(crate) cache_tip: String,
    pub(crate) scope: Option<SearchScopeInfo>,
    pub(crate) limit: usize,
    pub(crate) matched_count: usize,
    pub(crate) truncated: bool,
    pub(crate) events: Vec<Event>,
}

pub(crate) struct FragmentReport {
    pub(crate) query_file: Vec<u8>,
    pub(crate) path: Option<Vec<u8>>,
    pub(crate) cache_tip: String,
    pub(crate) scope: Option<SearchScopeInfo>,
    pub(crate) limit: usize,
    pub(crate) matched_count: usize,
    pub(crate) truncated: bool,
    pub(crate) events: Vec<FragmentEvent>,
}

pub(crate) struct FragmentEvent {
    pub(crate) commit_id: String,
    pub(crate) timestamp: String,
    pub(crate) message: Vec<u8>,
    pub(crate) first_parent_id: String,
    pub(crate) old_path: Vec<u8>,
    pub(crate) status: String,
    pub(crate) new_path: Option<Vec<u8>>,
    pub(crate) occurrences: Vec<FragmentOccurrence>,
    pub(crate) patch: Option<PatchExcerpt>,
    pub(crate) same_commit_files: SameCommitFiles,
}

pub(crate) struct FragmentOccurrence {
    pub(crate) commit_id: String,
    pub(crate) path: Vec<u8>,
    pub(crate) direction: CodeDirection,
    pub(crate) start_line: usize,
    pub(crate) end_line: usize,
    pub(crate) content: Vec<u8>,
}

pub(crate) struct Event {
    pub(crate) commit_id: String,
    pub(crate) timestamp: String,
    pub(crate) message: Vec<u8>,
    pub(crate) first_parent_id: String,
    pub(crate) old_path: Vec<u8>,
    pub(crate) status: String,
    pub(crate) new_path: Option<Vec<u8>>,
    pub(crate) matches: Vec<CodeMatch>,
    pub(crate) patch: Option<PatchExcerpt>,
    pub(crate) same_commit_files: SameCommitFiles,
}

// Only event identities are retained during discovery, never omitted source lines.
struct EventSelection {
    limit: usize,
    seen: BTreeSet<(String, Vec<u8>)>,
    selected: BTreeSet<(Reverse<i64>, String, Vec<u8>)>,
}

impl EventSelection {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            seen: BTreeSet::new(),
            selected: BTreeSet::new(),
        }
    }

    fn record(&mut self, time: i64, oid: &str, path: &[u8]) {
        if self.seen.insert((oid.to_owned(), path.to_vec())) {
            self.selected
                .insert((Reverse(time), oid.to_owned(), path.to_vec()));
            if self.selected.len() > self.limit {
                self.selected.pop_last();
            }
        }
    }
    fn into_groups<T>(self) -> (usize, SelectedEventGroups<T>) {
        let matched_count = self.seen.len();
        let groups = self
            .selected
            .into_iter()
            .map(|(_, oid, path)| ((oid, path), Vec::new()))
            .collect();
        (matched_count, groups)
    }
}

pub(in crate::analysis) fn execute(
    query: String,
    path: Option<String>,
    options: crate::analysis::query::Options,
) -> Result<crate::analysis::query::Outcome, AppError> {
    use crate::analysis::query::{Context, QueryReport};

    let context = Context::open(options.scope)?;
    let report = run(
        &context.session,
        &query,
        path.as_deref(),
        options.limit,
        context.filter(),
    )?;
    Ok(context.finish(QueryReport::TraceRemoval(report)))
}

pub(in crate::analysis) fn execute_fragment(
    input: PathBuf,
    path: Option<String>,
    options: crate::analysis::query::Options,
) -> Result<crate::analysis::query::Outcome, AppError> {
    use crate::analysis::query::{Context, QueryReport};

    let fragment = Fragment::read(&input)?;
    let context = Context::open(options.scope)?;
    let report = run_fragments(
        &context.session,
        &fragment,
        &input,
        path.as_deref(),
        options.limit,
        context.filter(),
    )?;
    Ok(context.finish(QueryReport::TraceRemovalFragment(report)))
}

fn run_fragments(
    session: &QuerySession,
    fragment: &Fragment,
    input: &std::path::Path,
    path: Option<&str>,
    limit: usize,
    scope: Option<&SearchFilter>,
) -> Result<FragmentReport, AppError> {
    let normalized_path = path.map(super::normalize_git_path_string);
    let path_filter = normalized_path.as_deref().map(str::as_bytes);
    let mut selection = EventSelection::new(limit);
    let discover = |hunk: CodeHunk| {
        code_fragment::visit_hunk(
            &hunk,
            fragment,
            path_filter,
            Some(CodeDirection::Removed),
            |_| {
                let old_path = hunk
                    .old_path
                    .as_deref()
                    .expect("removed fragment has an old path");
                let key = (hunk.oid.clone(), old_path.to_vec());
                if !selection.seen.contains(&key)
                    && session
                        .removal_change(&hunk.oid, old_path)?
                        .first_parent_id
                        .is_some()
                {
                    selection.record(hunk.commit_time, &hunk.oid, old_path);
                }
                Ok(())
            },
        )
    };
    match scope {
        Some(scope) => session.scan_code_hunks_scoped(scope, discover)?,
        None => session.scan_code_hunks(discover)?,
    }

    let (matched_count, mut groups) = selection.into_groups::<FragmentOccurrence>();
    if !groups.is_empty() {
        let collect = |hunk: CodeHunk| {
            let Some(old_path) = hunk.old_path.as_deref() else {
                return Ok(());
            };
            let Some(occurrences) = groups.get_mut(&(hunk.oid.clone(), old_path.to_vec())) else {
                return Ok(());
            };
            code_fragment::visit_hunk(
                &hunk,
                fragment,
                path_filter,
                Some(CodeDirection::Removed),
                |span| {
                    occurrences.push(fragment_occurrence(&hunk, old_path, span));
                    Ok(())
                },
            )
        };
        match scope {
            Some(scope) => session.scan_code_hunks_scoped(scope, collect)?,
            None => session.scan_code_hunks(collect)?,
        }
    }

    let mut events = Vec::new();
    for ((commit_id, old_path), mut occurrences) in groups {
        let change = session.removal_change(&commit_id, &old_path)?;
        let Some(first_parent_id) = change.first_parent_id else {
            continue;
        };
        occurrences.sort_by(|a, b| {
            a.start_line
                .cmp(&b.start_line)
                .then_with(|| a.end_line.cmp(&b.end_line))
                .then_with(|| a.content.cmp(&b.content))
        });
        let same_commit_files = match session.other_file_changes(
            &commit_id,
            change.change_ordinal,
            &old_path,
            change.new_path.as_deref(),
            MAX_SAME_COMMIT_FILE_CHANGES,
        ) {
            Ok(summary) => SameCommitFiles {
                status: if summary.truncated {
                    SameCommitFileStatus::Truncated
                } else {
                    SameCommitFileStatus::Complete
                },
                files: summary.changes,
            },
            Err(_) => SameCommitFiles {
                status: SameCommitFileStatus::Unavailable,
                files: Vec::new(),
            },
        };
        let matching_lines = occurrences
            .iter()
            .flat_map(|occurrence| occurrence.start_line..=occurrence.end_line)
            .filter_map(|line| i64::try_from(line).ok())
            .collect::<HashSet<_>>();
        let patch = removal_patch_excerpt_at_lines(
            session,
            &commit_id,
            change.change_ordinal,
            &old_path,
            &matching_lines,
        )?;
        events.push((
            change.commit_time,
            FragmentEvent {
                commit_id,
                timestamp: super::timeline::format_timestamp(change.commit_time),
                message: change.message,
                first_parent_id,
                old_path,
                status: change.status,
                new_path: change.new_path,
                occurrences,
                patch: Some(patch),
                same_commit_files,
            },
        ));
    }
    events.sort_by(|(a_time, a), (b_time, b)| {
        b_time
            .cmp(a_time)
            .then_with(|| a.commit_id.cmp(&b.commit_id))
            .then_with(|| a.old_path.cmp(&b.old_path))
    });
    Ok(FragmentReport {
        query_file: input.as_os_str().as_encoded_bytes().to_vec(),
        path: path.map(|path| path.as_bytes().to_vec()),
        cache_tip: session.completed_tip()?,
        scope: None,
        limit,
        matched_count,
        truncated: matched_count > limit,
        events: events.into_iter().map(|(_, event)| event).collect(),
    })
}

fn fragment_occurrence(hunk: &CodeHunk, path: &[u8], span: Span<'_>) -> FragmentOccurrence {
    let first = span.rows.first().expect("non-empty fragment span");
    let last = span.rows.last().expect("non-empty fragment span");
    FragmentOccurrence {
        commit_id: hunk.oid.clone(),
        path: path.to_vec(),
        direction: span.direction,
        start_line: first.number,
        end_line: last.number,
        content: span
            .rows
            .iter()
            .flat_map(|row| row.content.iter().copied())
            .collect(),
    }
}

fn run(
    session: &QuerySession,
    query: &str,
    path: Option<&str>,
    limit: usize,
    scope: Option<&SearchFilter>,
) -> Result<Report, AppError> {
    let mut selection = EventSelection::new(limit);
    super::code_search::visit_matches(
        session,
        query,
        path,
        CodeDirection::Removed,
        scope,
        |hunk, _, _| {
            let old_path = hunk
                .old_path
                .as_deref()
                .expect("removed line has an old path");
            let key = (hunk.oid.clone(), old_path.to_vec());
            if !selection.seen.contains(&key)
                && session
                    .removal_change(&hunk.oid, old_path)?
                    .first_parent_id
                    .is_some()
            {
                selection.record(hunk.commit_time, &hunk.oid, old_path);
            }
            Ok(())
        },
    )?;
    let (matched_count, mut groups) = selection.into_groups::<CodeMatch>();
    // A second pass retains complete line evidence only for the selected events.
    if !groups.is_empty() {
        super::code_search::visit_matches(
            session,
            query,
            path,
            CodeDirection::Removed,
            scope,
            |hunk, line_number, line| {
                let old_path = hunk
                    .old_path
                    .as_deref()
                    .expect("removed line has an old path");
                if let Some(matches) = groups.get_mut(&(hunk.oid.clone(), old_path.to_vec())) {
                    matches.push(CodeMatch {
                        commit_id: hunk.oid.clone(),
                        path: old_path.to_vec(),
                        direction: CodeDirection::Removed,
                        line_number,
                        line: line.to_vec(),
                    });
                }
                Ok(())
            },
        )?;
    }
    let mut events = Vec::new();
    for ((commit_id, old_path), mut matches) in groups {
        let change = session.removal_change(&commit_id, &old_path)?;
        let Some(first_parent_id) = change.first_parent_id else {
            continue;
        };
        matches.sort_by(|a, b| {
            a.line_number
                .cmp(&b.line_number)
                .then_with(|| a.line.cmp(&b.line))
        });
        matches.dedup_by(|a, b| a.line_number == b.line_number && a.line == b.line);

        let same_commit_files = match session.other_file_changes(
            &commit_id,
            change.change_ordinal,
            &old_path,
            change.new_path.as_deref(),
            MAX_SAME_COMMIT_FILE_CHANGES,
        ) {
            Ok(summary) => SameCommitFiles {
                status: if summary.truncated {
                    SameCommitFileStatus::Truncated
                } else {
                    SameCommitFileStatus::Complete
                },
                files: summary.changes,
            },
            Err(_) => SameCommitFiles {
                status: SameCommitFileStatus::Unavailable,
                files: Vec::new(),
            },
        };
        let patch = removal_patch_excerpt(
            session,
            &commit_id,
            change.change_ordinal,
            &old_path,
            &matches,
        )?;
        events.push((
            change.commit_time,
            Event {
                commit_id,
                timestamp: super::timeline::format_timestamp(change.commit_time),
                message: change.message,
                first_parent_id,
                old_path,
                status: change.status,
                new_path: change.new_path,
                matches,
                patch: Some(patch),
                same_commit_files,
            },
        ));
    }
    events.sort_by(|(a_time, a), (b_time, b)| {
        b_time
            .cmp(a_time)
            .then_with(|| a.commit_id.cmp(&b.commit_id))
            .then_with(|| a.old_path.cmp(&b.old_path))
    });
    Ok(Report {
        query: query.to_owned(),
        path: path.map(|path| path.as_bytes().to_vec()),
        cache_tip: session.completed_tip()?,
        scope: None,
        limit,
        matched_count,
        truncated: matched_count > limit,
        events: events.into_iter().map(|(_, event)| event).collect(),
    })
}

fn removal_patch_excerpt(
    session: &QuerySession,
    commit_id: &str,
    change_ordinal: i64,
    old_path: &[u8],
    matches: &[CodeMatch],
) -> Result<PatchExcerpt, AppError> {
    let matching_lines = matches
        .iter()
        .filter(|matched| {
            matched.direction == CodeDirection::Removed && matched.path.as_slice() == old_path
        })
        .filter_map(|matched| i64::try_from(matched.line_number).ok())
        .collect::<HashSet<_>>();
    removal_patch_excerpt_at_lines(
        session,
        commit_id,
        change_ordinal,
        old_path,
        &matching_lines,
    )
}

fn removal_patch_excerpt_at_lines(
    session: &QuerySession,
    commit_id: &str,
    change_ordinal: i64,
    old_path: &[u8],
    matching_lines: &HashSet<i64>,
) -> Result<PatchExcerpt, AppError> {
    let history = session.patch_history_for_change_at_lines(
        commit_id,
        change_ordinal,
        matching_lines,
        MAX_SCANNED_HUNKS,
        MAX_CACHED_HUNK_BYTES,
    )?;
    let mut truncated = history.truncated;
    let mut remaining_bytes = MAX_RESULT_EXCERPT_BYTES;
    let mut hunks = Vec::new();
    'cached: for cached in history.hunks {
        if cached.old_path.as_deref() != Some(old_path) {
            continue;
        }
        let Some(text) = cached.text.as_deref() else {
            continue;
        };
        for fragment in trace_removal_fragments(&cached, text, matching_lines) {
            if hunks.len() == MAX_EXCERPTS || remaining_bytes == 0 {
                truncated = true;
                break 'cached;
            }
            let excerpt = bounded_patch_hunk(fragment, &mut remaining_bytes);
            truncated |= excerpt.truncated;
            hunks.push(excerpt);
        }
    }
    Ok(PatchExcerpt {
        commit_oid: commit_id.to_owned(),
        status: if hunks.is_empty() {
            PatchStatus::Unavailable
        } else {
            PatchStatus::Available
        },
        hunks,
        truncated,
    })
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
    let old_lines = rows[start..=end]
        .iter()
        .filter(|row| row.consumes_old)
        .count() as i64;
    let new_lines = rows[start..=end]
        .iter()
        .filter(|row| row.consumes_new)
        .count() as i64;
    let old_start = if old_lines == 0 {
        first.old_before.saturating_sub(1)
    } else {
        first.old_before
    };
    let new_start = if new_lines == 0 {
        first.new_before.saturating_sub(1)
    } else {
        first.new_before
    };
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

#[cfg(test)]
mod tests {
    use super::EventSelection;

    #[test]
    fn selection_counts_all_events_but_retains_only_the_limit_and_no_line_payloads() {
        let mut selection = EventSelection::new(2);
        for time in 0..100 {
            for _ in 0..50 {
                selection.record(time, &time.to_string(), b"a.rs");
            }
            assert!(selection.selected.len() <= 2);
        }
        assert_eq!(selection.seen.len(), 100);
        let ids: Vec<_> = selection
            .selected
            .iter()
            .map(|(_, oid, _)| oid.as_str())
            .collect();
        assert_eq!(ids, ["99", "98"]);
    }
}
