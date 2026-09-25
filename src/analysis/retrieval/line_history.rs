//! Interpret cached diff hunks while walking a line backward through path history.

use std::collections::HashSet;

use super::{HistoryCommit, HistoryHunk};

/// Trace a line into the parent version; return whether this commit added that line.
pub(in crate::analysis) fn trace_line(
    commit: &HistoryCommit,
    hunks: &[HistoryHunk],
    line: &mut i64,
) -> bool {
    let relevant = commit
        .anchored_ordinals
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    let mut ordered = hunks
        .iter()
        .filter(|hunk| relevant.contains(&hunk.change_ordinal))
        .collect::<Vec<_>>();
    ordered.sort_by_key(|hunk| std::cmp::Reverse(hunk.new_start));
    for hunk in ordered {
        let new_end = hunk.new_start + hunk.new_lines - 1;
        if *line > new_end {
            *line += hunk.old_lines - hunk.new_lines;
            continue;
        }
        if *line < hunk.new_start || hunk.new_lines == 0 {
            continue;
        }
        let mut old_line = hunk.old_start;
        let mut new_line = hunk.new_start;
        let mut deleted_start = None;
        let mut direct = false;
        for diff_line in hunk.text.split_inclusive(|byte| *byte == b'\n') {
            let Some(marker) = diff_line.first() else {
                continue;
            };
            match marker {
                b'+' => {
                    if new_line == *line {
                        direct = true;
                        *line = deleted_start.unwrap_or(old_line);
                    }
                    new_line += 1;
                }
                b'-' => {
                    deleted_start.get_or_insert(old_line);
                    old_line += 1;
                }
                b' ' => {
                    if new_line == *line {
                        *line = old_line;
                    }
                    old_line += 1;
                    new_line += 1;
                    deleted_start = None;
                }
                b'\\' | b'@' => {}
                _ => {}
            }
        }
        return direct;
    }
    false
}

/// Check whether changed diff lines touch a symbol's current line range.
pub(in crate::analysis) fn hunk_overlaps_symbol(hunk: &HistoryHunk, start: i64, end: i64) -> bool {
    let mut line = hunk.new_start;
    for diff_line in hunk.text.split_inclusive(|byte| *byte == b'\n') {
        let Some(marker) = diff_line.first() else {
            continue;
        };
        match marker {
            b'+' => {
                if (start..=end).contains(&line) {
                    return true;
                }
                line += 1;
            }
            b'-' => {
                if (start..=end).contains(&line) {
                    return true;
                }
            }
            b' ' => line += 1,
            b'\\' | b'@' => {}
            _ => {}
        }
    }
    false
}
