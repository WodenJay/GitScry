//! Fate target pinning: a validated historical line or symbol at one revision.

use super::{read_shallow_boundaries, read_tree_entry, resolve_revision, validate_path};
use crate::app::AppError;
use crate::git::process::Git;
use crate::git::symbol;

pub(crate) struct FateTarget {
    pub(crate) revision: String,
    pub(crate) path: Vec<u8>,
    pub(crate) line: usize,
    /// Present when the target was selected by `--symbol`.
    pub(crate) symbol_selection: Option<symbol::Selection>,
    /// One-based end line of the symbol span; `None` for line targets.
    pub(crate) symbol_end_line: Option<usize>,
    pub(crate) warnings: Vec<String>,
}

pub(in crate::git) fn pin(
    git: &Git,
    revision: &str,
    path: &str,
    line: Option<usize>,
    symbol_name: Option<&str>,
) -> Result<FateTarget, AppError> {
    let path = validate_path(path)?;
    if revision.contains('\0') {
        return Err(AppError::input("revision must not contain a NUL byte"));
    }
    let revision = resolve_revision(git, revision)?;
    let shallow = !read_shallow_boundaries(git)?.is_empty();
    let mut warnings = Vec::new();
    if shallow {
        warnings
            .push("warning: local history is shallow; fate material may be incomplete.".to_owned());
    }
    let entry = read_tree_entry(git, &revision, &path)?;
    if entry.kind != "blob" {
        return Err(AppError::input(format!(
            "path is not a file at revision: {path}"
        )));
    }
    let content = git
        .output(["cat-file", "blob", &format!("{}:{path}", revision)], &[])
        .map_err(|error| {
            AppError::operational(format!("error: reading target path at {revision}: {error}"))
        })?;
    let line_count = if content.is_empty() {
        0
    } else {
        let count = content.split(|byte| *byte == b'\n').count();
        if content.ends_with(b"\n") {
            count.saturating_sub(1)
        } else {
            count
        }
    };
    let (line, symbol_selection, symbol_end_line) = if let Some(name) = symbol_name {
        let location = symbol::locate_unique(&content, name, &path)?;
        if let Some(notice) = &location.notice {
            warnings.push(notice.clone());
        }
        let end = location.span.end;
        (location.span.start, Some(location.selection), Some(end))
    } else {
        let line = line.unwrap_or(0);
        if line == 0 || line > line_count {
            return Err(AppError::input(format!(
                "line {line} is outside {path} at the starting revision"
            )));
        }
        (line, None, None)
    };
    Ok(FateTarget {
        revision,
        path: path.as_bytes().to_vec(),
        line,
        symbol_selection,
        symbol_end_line,
        warnings,
    })
}
