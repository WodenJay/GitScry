//! Why target pinning and line or symbol anchors.

use super::blame::{Blame, blame_ignore_file, read_blame};
use super::{read_shallow_boundaries, read_tree_entry, resolve_revision, validate_path};
use crate::app::AppError;
use crate::git::{SymbolTrace, process::Git, symbol};
#[derive(Clone)]
pub(crate) enum WhyAnchor {
    Line { number: usize },
    Symbol { name: String, number: usize },
}

pub(crate) struct WhyTarget {
    pub(crate) revision: String,
    pub(crate) path: Vec<u8>,
    pub(crate) anchor: WhyAnchor,
    pub(crate) symbol_end: Option<usize>,
    pub(crate) symbol_selection: Option<symbol::Selection>,
    pub(crate) blame: Option<Blame>,
    pub(crate) warnings: Vec<String>,
    pub(crate) anchor_valid: bool,
    pub(crate) symbol_trace: Option<SymbolTrace>,
}

pub(in crate::git) fn pin(
    git: &Git,
    revision: Option<&str>,
    path: &str,
    anchor: WhyAnchor,
) -> Result<WhyTarget, AppError> {
    let path = validate_path(path)?;
    let requested_revision = revision.unwrap_or("HEAD");
    if requested_revision.contains('\0') {
        return Err(AppError::input("revision must not contain a NUL byte"));
    }
    let revision = resolve_revision(git, requested_revision)?;
    let shallow = !read_shallow_boundaries(git)?.is_empty();
    let mut warnings = Vec::new();
    if shallow {
        warnings
            .push("warning: local history is shallow; why material may be incomplete.".to_owned());
    }

    let entry = read_tree_entry(git, &revision, &path)?;
    if entry.kind == "commit" {
        warnings.push(
            "warning: target path is a submodule; history stops at the submodule boundary."
                .to_owned(),
        );
        return Ok(WhyTarget {
            revision,
            path: path.as_bytes().to_vec(),
            anchor,
            symbol_end: None,
            symbol_selection: None,
            blame: None,
            anchor_valid: false,
            symbol_trace: None,
            warnings,
        });
    }
    if entry.kind != "blob" {
        return Err(AppError::input(format!(
            "path is not a file at revision: {path}"
        )));
    }

    let content = git
        .output(["cat-file", "blob", &format!("{}:{path}", revision)], &[])
        .map_err(|error| {
            AppError::operational(format!(
                "error: reading target path at {requested_revision}: {error}"
            ))
        })?;
    let (anchor, number, location) = resolve_anchor(&anchor, &content, &path)?;
    let symbol_end = location.as_ref().map(|location| location.span.end);
    let symbol_selection = location.as_ref().map(|location| location.selection.clone());
    if let Some(notice) = location.and_then(|location| location.notice) {
        warnings.push(notice);
    }
    let ignore_file = blame_ignore_file(git)?;
    let (blame, used_ignore_file) =
        read_blame(git, &revision, &path, number, ignore_file.as_deref())?;
    if blame.is_none() {
        warnings.push(
            "warning: Git line attribution unavailable; explanation ranking uses cached history."
                .to_owned(),
        );
    }
    if used_ignore_file {
        warnings.push(
            "warning: blame ignored revisions from .git-blame-ignore-revs; attribution may be incomplete."
                .to_owned(),
        );
    }
    let blame = blame.inspect(|blame| {
        if shallow && blame.boundary {
            warnings.push(
                "warning: blame reached a local history boundary; the original explanation may be incomplete."
                    .to_owned(),
            );
        }
    });

    Ok(WhyTarget {
        revision,
        path: path.as_bytes().to_vec(),
        anchor,
        symbol_end,
        symbol_selection,
        blame,
        anchor_valid: true,
        symbol_trace: None,
        warnings,
    })
}

fn resolve_anchor(
    anchor: &WhyAnchor,
    content: &[u8],
    path: &str,
) -> Result<(WhyAnchor, usize, Option<symbol::Location>), AppError> {
    match anchor {
        WhyAnchor::Line { number } => {
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
            if *number == 0 || *number > line_count {
                return Err(AppError::input(format!(
                    "line {number} is outside {path} at the target revision"
                )));
            }
            Ok((anchor.clone(), *number, None))
        }
        WhyAnchor::Symbol { name, .. } => {
            let location = symbol::locate_unique(content, name, path)?;
            let start = location.span.start;
            Ok((
                WhyAnchor::Symbol {
                    name: name.clone(),
                    number: start,
                },
                start,
                Some(location),
            ))
        }
    }
}
