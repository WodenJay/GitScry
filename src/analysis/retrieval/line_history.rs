//! Interpret cached diff hunks while walking a line backward through path history.

use crate::cache::HunkId;
use std::collections::HashSet;

use super::{HistoryCommit, HistoryHunk};

/// Trace a line into the parent version; return whether this commit added that line.
pub(in crate::analysis) fn trace_line(
    commit: &HistoryCommit,
    hunks: &[HistoryHunk],
    line: &mut i64,
) -> Option<HunkId> {
    let relevant = commit
        .anchored_ordinals
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    let relevant_hunks = hunks
        .iter()
        .filter(|hunk| relevant.contains(&hunk.change_ordinal));
    let (changed, parent, _) = map_line(relevant_hunks, *line, true)?;
    *line = parent;
    changed
}

/// Check whether changed diff lines touch a symbol's current line range.
pub(in crate::analysis) fn hunk_overlaps_symbol(hunk: &HistoryHunk, start: i64, end: i64) -> bool {
    let mut new_line = hunk.new_start;
    for diff_line in hunk.text.split_inclusive(|byte| *byte == b'\n') {
        let Some(marker) = diff_line.first() else {
            continue;
        };
        match marker {
            b'+' => {
                if (start..=end).contains(&new_line) {
                    return true;
                }
                new_line += 1;
            }
            b'-' => {
                // The cursor is a boundary: `start` is just before the symbol.
                if new_line > start && new_line <= end {
                    return true;
                }
            }
            b' ' => new_line += 1,
            b'\\' | b'@' => {}
            _ => {}
        }
    }
    false
}
/// Return the changed-line fact, parent cursor and whether the line was introduced.
/// An introduced line keeps its parent boundary cursor for callers tracing beyond birth.
pub(in crate::analysis) fn map_line<'a>(
    hunks: impl IntoIterator<Item = &'a HistoryHunk>,
    line: i64,
    unchanged: bool,
) -> Option<(Option<HunkId>, i64, bool)> {
    let mut ordered = hunks.into_iter().collect::<Vec<_>>();
    if ordered.is_empty() {
        return unchanged.then_some((None, line, false));
    }
    ordered.sort_by_key(|hunk| hunk.new_start);
    let mut shift = 0;
    for hunk in ordered {
        let mut old = hunk.old_start;
        let mut new = hunk.new_start;
        let mut deleted = None;
        let mut mapped = None;
        let mut old_count = 0;
        let mut new_count = 0;
        for text in hunk.text.split_inclusive(|byte| *byte == b'\n') {
            match text.first() {
                Some(b'-') => {
                    deleted.get_or_insert(old);
                    old += 1;
                    old_count += 1;
                }
                Some(b'+') => {
                    if new == line {
                        mapped = Some((Some(hunk.id()), deleted.unwrap_or(old), deleted.is_none()));
                    }
                    new += 1;
                    new_count += 1;
                }
                Some(b' ') => {
                    if new == line {
                        mapped = Some((None, old, false));
                    }
                    old += 1;
                    new += 1;
                    old_count += 1;
                    new_count += 1;
                    deleted = None;
                }
                Some(b'@' | b'\\') => {}
                _ => return None,
            }
        }
        if old_count != hunk.old_lines || new_count != hunk.new_lines {
            return None;
        }
        if let Some(mapped) = mapped {
            return Some(mapped);
        }
        // A deletion's new_start is the preceding line; don't move that line.
        let end = hunk.new_start + hunk.new_lines - i64::from(hunk.new_lines > 0);
        if line > end {
            shift += hunk.old_lines - hunk.new_lines;
        }
    }
    Some((None, line + shift, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_insertion_keeps_the_parent_boundary_cursor_for_why() {
        let commit = HistoryCommit {
            position: 0,
            oid: String::new(),
            commit_time: 0,
            subject: String::new(),
            body: String::new(),
            paths: Vec::new(),
            changes: Vec::new(),
            anchored_ordinals: vec![0],
            parent_count: 1,
            shallow_boundary: false,
        };
        let hunk = HistoryHunk {
            change_ordinal: 0,
            hunk_ordinal: 0,
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 3,
            text: b"+before\n+target\n original\n".to_vec(),
        };
        let mut line = 2;
        assert_eq!(
            trace_line(&commit, &[hunk], &mut line),
            Some(HunkId {
                change_ordinal: 0,
                hunk_ordinal: 0
            })
        );
        assert_eq!(line, 1);
    }
}
