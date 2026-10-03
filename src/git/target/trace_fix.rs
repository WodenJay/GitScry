//! Fix target preparation and deleted-line attribution.

use super::blame::{Blame, blame_ignore_file, read_blame_range};
use super::{normalize_path, read_shallow_boundaries, resolve_revision, validate_path};
use crate::app::AppError;
use crate::git::{history, process::Git};
use std::collections::HashSet;
pub(crate) struct TraceFixTarget {
    pub(crate) revision: String,
    pub(crate) fix: history::Commit,
    pub(crate) parent: Option<String>,
    pub(crate) shallow: bool,
    pub(crate) deleted_lines: Vec<DeletedLine>,
    pub(crate) warnings: Vec<String>,
}

pub(crate) struct DeletedLine {
    pub(crate) path: Vec<u8>,
    pub(crate) line: usize,
    pub(crate) blame: Option<Blame>,
}

pub(in crate::git) fn pin_trace_fix(
    git: &Git,
    requested_revision: &str,
    paths: &[String],
) -> Result<TraceFixTarget, AppError> {
    let paths = paths
        .iter()
        .map(|path| validate_path(path))
        .collect::<Result<Vec<_>, _>>()?;
    if requested_revision.contains('\0') {
        return Err(AppError::input("revision must not contain a NUL byte"));
    }
    let revision = resolve_revision(git, requested_revision)?;
    let (fix, all_changes, hunks) = history::read_trace_fix(git, &revision)?;
    let parent = fix.parents.first().cloned();
    let shallow = !read_shallow_boundaries(git)?.is_empty();
    let mut warnings = Vec::new();
    let parent_missing = parent
        .as_ref()
        .map(|parent| git.missing_objects(std::slice::from_ref(parent)))
        .transpose()?
        .is_some_and(|missing| !missing.is_empty());
    if all_changes.is_none() {
        warnings.push(
            "warning: fix changed-path metadata is unavailable locally; lineage is degraded (confidence: low)."
                .to_owned(),
        );
    }
    if shallow {
        warnings.push(
            "warning: local history is shallow; trace-fix lineage may be incomplete.".to_owned(),
        );
    }
    if parent_missing {
        warnings.push(
            "warning: fix parent is missing locally; deleted-line lineage is unavailable (confidence: low)."
                .to_owned(),
        );
    }
    if fix.parents.len() > 1 {
        warnings.push(
            "warning: fix is a merge; first-parent deleted lines are used and merge lineage is ambiguous."
                .to_owned(),
        );
    }
    if parent.is_none() {
        warnings.push(
            "warning: fix has no parent; introducing lineage is unavailable (confidence: low)."
                .to_owned(),
        );
    }
    let selected_ordinals = match all_changes.as_ref() {
        None => HashSet::new(),
        Some(all_changes) if paths.is_empty() => all_changes
            .iter()
            .map(|change| change.ordinal)
            .collect::<HashSet<_>>(),
        Some(all_changes) => {
            let mut selected = HashSet::new();
            for path in &paths {
                let normalized = path.as_bytes();
                let matches = all_changes.iter().filter(|change| {
                    change.old_path.as_deref().map(normalize_path).as_deref() == Some(normalized)
                        || change.new_path.as_deref().map(normalize_path).as_deref()
                            == Some(normalized)
                });
                let mut found = false;
                for change in matches {
                    found = true;
                    selected.insert(change.ordinal);
                }
                if !found {
                    return Err(AppError::input(format!(
                        "path was not changed by fix revision: {path}"
                    )));
                }
            }
            selected
        }
    };
    let changes = all_changes
        .unwrap_or_default()
        .into_iter()
        .filter(|change| selected_ordinals.contains(&change.ordinal))
        .collect::<Vec<_>>();
    let object_ids = changes
        .iter()
        .flat_map(|change| [&change.old_blob, &change.new_blob].into_iter().flatten())
        .cloned()
        .collect::<Vec<_>>();
    if !git.missing_objects(&object_ids)?.is_empty() {
        warnings.push(
            "warning: fix diff blobs are missing locally; deleted-line lineage may be incomplete (confidence: low)."
                .to_owned(),
        );
    }
    let hunks_available = hunks.is_some();
    if !hunks_available {
        warnings.push(
            "warning: fix diff hunks are unavailable locally; deleted-line lineage may be incomplete (confidence: low)."
                .to_owned(),
        );
    }
    let mut deleted_lines = Vec::new();
    if let (Some(parent), Some(hunks)) = (parent.as_deref(), hunks.as_ref()) {
        for hunk in hunks {
            if !selected_ordinals.contains(&hunk.change_ordinal) {
                continue;
            }
            let Some(path) = changes
                .iter()
                .find(|change| change.ordinal == hunk.change_ordinal)
                .and_then(|change| change.old_path.as_deref())
            else {
                continue;
            };
            let mut old_line = hunk.old_start;
            for diff_line in hunk.text.split_inclusive(|byte| *byte == b'\n') {
                match diff_line.first() {
                    Some(b'-') => {
                        if let Ok(line) = usize::try_from(old_line) {
                            deleted_lines.push(DeletedLine {
                                path: path.to_vec(),
                                line,
                                blame: None,
                            });
                        }
                        old_line += 1;
                    }
                    Some(b' ') => old_line += 1,
                    _ => {}
                }
            }
        }
        let ignore_file = blame_ignore_file(git)?;
        let mut start = 0;
        while start < deleted_lines.len() {
            let mut end = start + 1;
            while end < deleted_lines.len()
                && deleted_lines[end].path == deleted_lines[start].path
                && deleted_lines[end].line == deleted_lines[end - 1].line + 1
            {
                end += 1;
            }
            let path = String::from_utf8_lossy(&deleted_lines[start].path);
            let (mut blamed, used_ignore_file) = read_blame_range(
                git,
                parent,
                &path,
                deleted_lines[start].line,
                deleted_lines[end - 1].line,
                ignore_file.as_deref(),
            )?;
            if used_ignore_file {
                push_warning(
                    &mut warnings,
                    "warning: blame ignored revisions from .git-blame-ignore-revs; trace-fix attribution may be incomplete."
                        .to_owned(),
                );
            }
            for deleted in &mut deleted_lines[start..end] {
                deleted.blame = blamed.remove(&deleted.line);
                if let Some(blame) = &deleted.blame {
                    if shallow && blame.boundary {
                        push_warning(
                            &mut warnings,
                            "warning: fix-parent blame reached a shallow boundary; introducing lineage may be incomplete (confidence: low)."
                                .to_owned(),
                        );
                    }
                } else {
                    push_warning(
                        &mut warnings,
                        "warning: fix-parent deleted-line blame is unavailable; introducing lineage may be incomplete (confidence: low)."
                            .to_owned(),
                    );
                }
            }
            start = end;
        }
    }
    if hunks_available && deleted_lines.is_empty() {
        warnings.push(
            "warning: fix has no deleted lines; introducing lineage is unavailable (confidence: low)."
                .to_owned(),
        );
    }
    Ok(TraceFixTarget {
        revision,
        parent,
        shallow,
        fix,
        deleted_lines,
        warnings,
    })
}

fn push_warning(warnings: &mut Vec<String>, warning: String) {
    if !warnings.contains(&warning) {
        warnings.push(warning);
    }
}
