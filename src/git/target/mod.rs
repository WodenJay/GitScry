//! Shared Git target operations and feature-specific target preparation.

use std::path::Path;

use super::process::Git;
use crate::app::AppError;

mod blame;
mod regression;
mod timeline;
mod trace_fix;
mod why;

pub(crate) use regression::RegressionTarget;
pub(super) use regression::pin_regression;
pub(crate) use timeline::TimelineTarget;
pub(super) use timeline::pin_timeline;
pub(super) use trace_fix::pin_trace_fix;
pub(crate) use trace_fix::{DeletedLine, TraceFixTarget};
pub(super) use why::pin;
pub(crate) use why::{WhyAnchor, WhyTarget};

fn normalize_path(path: &[u8]) -> Vec<u8> {
    let mut normalized = Vec::new();
    for component in path.split(|byte| *byte == b'/' || *byte == b'\\') {
        if component.is_empty() || component == b"." {
            continue;
        }
        if !normalized.is_empty() {
            normalized.push(b'/');
        }
        normalized.extend_from_slice(component);
    }
    normalized
}

fn validate_path(path: &str) -> Result<String, AppError> {
    if path.is_empty() {
        return Err(AppError::input("path must not be empty"));
    }
    if path.contains('\0') {
        return Err(AppError::input("path must not contain a NUL byte"));
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return Err(AppError::input(format!(
            "path must be repository-relative: {path}"
        )));
    }

    let normalized = path
        .split(['/', '\\'])
        .filter(|component| !component.is_empty() && *component != ".")
        .collect::<Vec<_>>()
        .join("/");
    if normalized.is_empty() {
        return Err(AppError::input(format!(
            "path must name a repository file: {path}"
        )));
    }

    let value = Path::new(&normalized);

    #[cfg(windows)]
    let has_prefix = value
        .components()
        .any(|component| matches!(component, std::path::Component::Prefix(_)));
    #[cfg(not(windows))]
    let has_prefix = false;
    if has_prefix
        || value.is_absolute()
        || value.has_root()
        || value
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(AppError::input(format!(
            "path must be repository-relative: {path}"
        )));
    }

    Ok(normalized)
}

pub(super) fn resolve_revision(git: &Git, requested: &str) -> Result<String, AppError> {
    let spec = format!("{requested}^{{commit}}");
    let result = git.text([
        "rev-parse",
        "--verify",
        "--quiet",
        "--end-of-options",
        &spec,
    ]);
    match result {
        Ok(value) => Ok(value.trim().to_owned()),
        Err(_) => Err(AppError::input(format!("invalid revision: {requested}"))),
    }
}

struct TreeEntry {
    kind: String,
}

fn read_tree_entry(git: &Git, revision: &str, path: &str) -> Result<TreeEntry, AppError> {
    let output = git.output(
        [
            "--literal-pathspecs",
            "ls-tree",
            "-z",
            "--full-tree",
            revision,
            "--",
            path,
        ],
        &[],
    )?;
    for entry in output
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let Some(tab) = entry.iter().position(|byte| *byte == b'\t') else {
            return Err(AppError::operational(
                "error: Git tree entry had an unexpected shape",
            ));
        };
        let (header, entry_path) = entry.split_at(tab);
        let entry_path = &entry_path[1..];
        if entry_path == path.as_bytes() {
            let mut fields = header.split(|byte| *byte == b' ');
            let _mode = fields.next();
            let kind = fields
                .next()
                .and_then(|value| std::str::from_utf8(value).ok())
                .ok_or_else(|| {
                    AppError::operational("error: Git tree entry omitted its object type")
                })?;
            return Ok(TreeEntry {
                kind: kind.to_owned(),
            });
        }
    }
    Err(AppError::input(format!(
        "path does not exist at revision: {path}"
    )))
}

pub(super) fn read_blob_at(
    git: &Git,
    revision: &str,
    path: &str,
) -> Result<Option<Vec<u8>>, AppError> {
    let entry = match read_tree_entry(git, revision, path) {
        Ok(entry) => entry,
        Err(error) if error.exit_code() == 2 => return Ok(None),
        Err(error) => return Err(error),
    };
    if entry.kind != "blob" {
        return Ok(None);
    }
    git.output(["cat-file", "blob", &format!("{revision}:{path}")], &[])
        .map(Some)
}

fn read_shallow_boundaries(git: &Git) -> Result<Vec<String>, AppError> {
    if git.text(["rev-parse", "--is-shallow-repository"])?.trim() != "true" {
        return Ok(Vec::new());
    }
    let path = git.text(["rev-parse", "--git-path", "shallow"])?;
    let path = Path::new(path.trim());
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        git.root().join(path)
    };
    let contents = std::fs::read_to_string(path).map_err(|error| {
        AppError::operational(format!("error: reading shallow boundaries: {error}"))
    })?;
    Ok(contents.lines().map(str::to_owned).collect())
}

fn is_oid(value: &[u8]) -> bool {
    matches!(value.len(), 40 | 64) && value.iter().all(u8::is_ascii_hexdigit)
}
