//! Regression target pinning.

use super::{read_shallow_boundaries, read_tree_entry, resolve_revision, validate_path};
use crate::app::AppError;
use crate::git::{process::Git, symbol};
pub(crate) struct RegressionTarget {
    pub(crate) bad_revision: String,
    pub(crate) good_revision: Option<String>,
    pub(crate) path: Vec<u8>,
    pub(crate) symbol: Option<String>,
    pub(crate) symbol_line: Option<usize>,
    pub(crate) symbol_end: Option<usize>,
    pub(crate) warnings: Vec<String>,
}

pub(in crate::git) fn pin_regression(
    git: &Git,
    bad_revision: &str,
    good_revision: Option<&str>,
    path: &str,
    symbol: Option<&str>,
) -> Result<RegressionTarget, AppError> {
    let path = validate_path(path)?;
    let bad_revision = resolve_revision(git, bad_revision)?;
    let good_revision = good_revision
        .map(|revision| resolve_revision(git, revision))
        .transpose()?;
    if let Some(good_revision) = &good_revision
        && (good_revision == &bad_revision
            || !git.success(["merge-base", "--is-ancestor", good_revision, &bad_revision])?)
    {
        return Err(AppError::input(
            "good revision must be an ancestor of bad revision",
        ));
    }

    let entry = read_tree_entry(git, &bad_revision, &path)?;
    if entry.kind != "blob" {
        return Err(AppError::input(format!(
            "path is not a file at bad revision: {path}",
        )));
    }
    let content = git.output(["cat-file", "blob", &format!("{bad_revision}:{path}")], &[])?;
    let (symbol, symbol_line, symbol_end) = match symbol {
        Some(name) => {
            let span = symbol::locate(&content, name, &path)?;
            (Some(name.to_owned()), Some(span.start), Some(span.end))
        }
        None => (None, None, None),
    };

    let shallow = read_shallow_boundaries(git)?;
    let mut warnings = Vec::new();
    if !shallow.is_empty() {
        warnings.push(
            "warning: local history is shallow; regression material may be incomplete.".to_owned(),
        );
    }

    Ok(RegressionTarget {
        bad_revision,
        good_revision,
        path: path.as_bytes().to_vec(),
        symbol,
        symbol_line,
        symbol_end,
        warnings,
    })
}
