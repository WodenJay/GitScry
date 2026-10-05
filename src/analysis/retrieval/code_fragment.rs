//! Byte-exact fragment input and changed-side continuity, shared by historical capabilities.
use std::{io::Read, path::Path};

use crate::{analysis::CodeDirection, app::AppError, cache::CodeHunk};

pub(crate) struct Fragment {
    lines: Vec<Vec<u8>>,
}

impl Fragment {
    pub(crate) fn read(path: &Path) -> Result<Self, AppError> {
        let mut bytes = Vec::new();
        let result = if path == Path::new("-") {
            std::io::stdin().lock().read_to_end(&mut bytes)
        } else {
            std::fs::File::open(path).and_then(|mut file| file.read_to_end(&mut bytes))
        };
        result.map_err(|error| {
            AppError::input(format!(
                "cannot read code fragment {}: {error}",
                path.display()
            ))
        })?;
        if bytes.is_empty() {
            return Err(AppError::input("code fragment input is empty"));
        }
        let lines = bytes
            .split_inclusive(|byte| *byte == b'\n')
            .map(|raw| logical_line(raw).to_vec())
            .collect();
        Ok(Self { lines })
    }
}

fn logical_line(raw: &[u8]) -> &[u8] {
    match raw.strip_suffix(b"\n") {
        Some(line) => line.strip_suffix(b"\r").unwrap_or(line),
        None => raw,
    }
}

pub(in crate::analysis) struct SourceRow<'a> {
    pub(in crate::analysis) number: usize,
    pub(in crate::analysis) order: usize,
    /// Original source bytes, including an actual line ending, but no diff prefix.
    pub(in crate::analysis) content: &'a [u8],
}

pub(in crate::analysis) struct Span<'a> {
    pub(in crate::analysis) direction: CodeDirection,
    pub(in crate::analysis) rows: &'a [SourceRow<'a>],
    pub(in crate::analysis) diff_rows: &'a [&'a [u8]],
}

pub(in crate::analysis) fn visit_hunk(
    hunk: &CodeHunk,
    fragment: &Fragment,
    path: Option<&[u8]>,
    direction: Option<CodeDirection>,
    mut visit: impl FnMut(Span<'_>) -> Result<(), AppError>,
) -> Result<(), AppError> {
    let diff_rows: Vec<_> = hunk.text.split_inclusive(|byte| *byte == b'\n').collect();
    for side in [CodeDirection::Added, CodeDirection::Removed] {
        let historical_path = match side {
            CodeDirection::Added => hunk.new_path.as_deref(),
            CodeDirection::Removed => hunk.old_path.as_deref(),
        };
        if direction.is_some_and(|filter| filter != side)
            || historical_path.is_none()
            || path.is_some_and(|filter| Some(filter) != historical_path)
        {
            continue;
        }
        let start = match side {
            CodeDirection::Added => hunk.new_start,
            CodeDirection::Removed => hunk.old_start,
        };
        let mut number = usize::try_from(start)
            .map_err(|_| AppError::operational("error: cache contains an invalid hunk line"))?;
        let prefix = match side {
            CodeDirection::Added => b'+',
            CodeDirection::Removed => b'-',
        };
        let mut run = Vec::new();
        let mut flush = |run: &mut Vec<SourceRow<'_>>| -> Result<(), AppError> {
            for rows in run.windows(fragment.lines.len()) {
                if rows
                    .iter()
                    .zip(&fragment.lines)
                    .all(|(row, query)| logical_line(row.content) == query)
                {
                    visit(Span {
                        direction: side,
                        rows,
                        diff_rows: &diff_rows,
                    })?;
                }
            }
            run.clear();
            Ok(())
        };
        for (order, raw) in diff_rows.iter().enumerate() {
            if raw.first() == Some(&b' ') {
                flush(&mut run)?;
            } else if raw.first() != Some(&prefix) {
                // Opposite-direction changes and diff metadata consume no position on this side.
                continue;
            } else {
                let mut content = &raw[1..];
                if diff_rows
                    .get(order + 1)
                    .is_some_and(|next| next.starts_with(b"\\ No newline at end of file"))
                {
                    content = content.strip_suffix(b"\n").unwrap_or(content);
                }
                run.push(SourceRow {
                    number,
                    order,
                    content,
                });
            }
            number = number
                .checked_add(1)
                .ok_or_else(|| AppError::operational("error: cached hunk line number overflow"))?;
        }
        flush(&mut run)?;
    }
    Ok(())
}
