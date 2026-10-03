//! Timeline target pinning.

use super::{read_tree_entry, resolve_revision, validate_path};
use crate::app::AppError;
use crate::git::process::Git;
pub(crate) struct TimelineTarget {
    pub(crate) revision: String,
    pub(crate) path: Vec<u8>,
}

pub(in crate::git) fn pin_timeline(
    git: &Git,
    requested_revision: &str,
    path: &str,
) -> Result<TimelineTarget, AppError> {
    let path = validate_path(path)?;
    if requested_revision.contains('\0') {
        return Err(AppError::input("revision must not contain a NUL byte"));
    }
    let revision = resolve_revision(git, requested_revision)?;
    let entry = read_tree_entry(git, &revision, &path)?;
    if entry.kind != "blob" {
        return Err(AppError::input(format!(
            "path is not a file at timeline revision: {path}"
        )));
    }
    Ok(TimelineTarget {
        revision,
        path: path.as_bytes().to_vec(),
    })
}
