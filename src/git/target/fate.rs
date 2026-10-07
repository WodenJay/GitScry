//! Fate target pinning: a validated historical line at one revision.

use super::{read_shallow_boundaries, read_tree_entry, resolve_revision, validate_path};
use crate::app::AppError;
use crate::git::process::Git;

pub(crate) struct FateTarget {
    pub(crate) revision: String,
    pub(crate) path: Vec<u8>,
    pub(crate) line: usize,
    pub(crate) warnings: Vec<String>,
}

pub(in crate::git) fn pin(
    git: &Git,
    revision: &str,
    path: &str,
    line: usize,
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
    if line == 0 || line > line_count {
        return Err(AppError::input(format!(
            "line {line} is outside {path} at the starting revision"
        )));
    }
    Ok(FateTarget {
        revision,
        path: path.as_bytes().to_vec(),
        line,
        warnings,
    })
}
