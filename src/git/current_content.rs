//! Bounded current diff acquisition. Git owns tracked content identity and filtering.
use super::{
    current::{ContentOmission, CurrentHunk, CurrentPath, current_regular_file, worktree_path},
    process::Git,
};
use crate::app::AppError;
use std::{ffi::OsString, io::Read, path::Path};

const FILE_LIMIT: usize = 128;
const BYTE_LIMIT: usize = 256 * 1024;
const TOTAL_LIMIT: usize = 4 * 1024 * 1024;

pub(super) fn read(
    git: &Git,
    root: &Path,
    changes: &[CurrentPath],
    staged: bool,
    has_head: bool,
) -> Result<(Vec<CurrentHunk>, Vec<ContentOmission>), AppError> {
    let mut hunks = Vec::new();
    let mut omissions = Vec::new();
    let mut total = 0;
    for (ordinal, change) in changes.iter().enumerate() {
        let path = change
            .new_path
            .as_ref()
            .or(change.old_path.as_ref())
            .expect("selected path");
        if ordinal >= FILE_LIMIT || total >= TOTAL_LIMIT {
            omissions.push(ContentOmission {
                path: path.clone(),
                reason: "current_content_budget",
            });
            continue;
        }
        let mut reason = None;
        // Inspect repository-visible modes, never symlink targets or gitlink contents.
        for side in change.old_path.iter().chain(&change.new_path) {
            let arg = worktree_path(Path::new(""), side).into_os_string();
            let index = git.output(
                [
                    OsString::from("--literal-pathspecs"),
                    "ls-files".into(),
                    "--stage".into(),
                    "-z".into(),
                    "--".into(),
                    arg.clone(),
                ],
                &[],
            )?;
            let head = if has_head {
                git.output(
                    [
                        OsString::from("--literal-pathspecs"),
                        "ls-tree".into(),
                        "-z".into(),
                        "HEAD".into(),
                        "--".into(),
                        arg,
                    ],
                    &[],
                )?
            } else {
                Vec::new()
            };
            for entries in [&index, &head] {
                if entries
                    .split(|b| *b == 0)
                    .any(|entry| entry.starts_with(b"120000 ") || entry.starts_with(b"160000 "))
                {
                    reason = Some("symlink_or_submodule");
                }
            }
        }
        if !staged && let Some(new) = &change.new_path {
            let full = worktree_path(root, new);
            if std::fs::symlink_metadata(&full).is_ok() && !current_regular_file(root, new) {
                reason = Some("non_regular_worktree_path");
            }
        }
        if let Some(reason) = reason {
            omissions.push(ContentOmission {
                path: path.clone(),
                reason,
            });
            continue;
        }
        let budget = BYTE_LIMIT.min(TOTAL_LIMIT - total);
        let bytes = if change.status == "untracked" || (!has_head && !staged) {
            let full = worktree_path(root, path);
            let file = std::fs::File::open(&full).map_err(|e| {
                AppError::operational(format!("error: reading current content: {e}"))
            })?;
            let mut bytes = Vec::new();
            file.take(budget as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| {
                    AppError::operational(format!("error: reading current content: {e}"))
                })?;
            (bytes.len() <= budget).then_some(bytes)
        } else {
            let mut args: Vec<OsString> = [
                "--literal-pathspecs",
                "diff",
                "--patch",
                "--unified=0",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--ignore-submodules=dirty",
            ]
            .into_iter()
            .map(Into::into)
            .collect();
            if staged {
                args.push("--cached".into());
            }
            if has_head {
                args.push("HEAD".into());
            }
            args.push("--".into());
            for side in change.old_path.iter().chain(&change.new_path) {
                args.push(worktree_path(Path::new(""), side).into_os_string());
            }
            git.limited_output(args, budget)?
        };
        let Some(bytes) = bytes else {
            // A failed bounded read consumes its allocation of the total budget too.
            total += budget;
            omissions.push(ContentOmission {
                path: path.clone(),
                reason: "oversized_content",
            });
            continue;
        };
        total += bytes.len();
        if bytes.contains(&0)
            || bytes.starts_with(b"\xff\xfe")
            || bytes.starts_with(b"\xfe\xff")
            || bytes.windows(13).any(|w| w == b"Binary files ")
        {
            omissions.push(ContentOmission {
                path: path.clone(),
                reason: "binary_content",
            });
            continue;
        }
        if std::str::from_utf8(&bytes).is_err() {
            omissions.push(ContentOmission {
                path: path.clone(),
                reason: "non_utf8_content",
            });
            continue;
        }
        if change.status == "untracked" || (!has_head && !staged) {
            hunks.push(CurrentHunk {
                path: path.clone(),
                added: true,
                old_start: 0,
                new_start: 1,
                text: bytes,
            });
        } else {
            parse_patch(&bytes, change, &mut hunks)?;
        }
    }
    Ok((hunks, omissions))
}

fn parse_patch(
    bytes: &[u8],
    change: &CurrentPath,
    hunks: &mut Vec<CurrentHunk>,
) -> Result<(), AppError> {
    let mut sides: Option<(CurrentHunk, CurrentHunk)> = None;
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        if line.starts_with(b"@@ ") {
            if let Some((removed, added)) = sides.take() {
                if !removed.text.is_empty() {
                    hunks.push(removed);
                }
                if !added.text.is_empty() {
                    hunks.push(added);
                }
            }
            let header = String::from_utf8_lossy(line);
            let fields = header.split_whitespace().collect::<Vec<_>>();
            let start = |field: Option<&&str>| -> Result<usize, AppError> {
                field
                    .and_then(|s| s.get(1..))
                    .and_then(|s| s.split(',').next())
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| AppError::operational("error: invalid current hunk header"))
            };
            let old_start = start(fields.get(1))?;
            let new_start = start(fields.get(2))?;
            let make = |added| CurrentHunk {
                path: if added {
                    change.new_path.clone()
                } else {
                    change.old_path.clone()
                }
                .unwrap_or_default(),
                added,
                old_start,
                new_start,
                text: Vec::new(),
            };
            sides = Some((make(false), make(true)));
        } else if let Some((removed, added)) = &mut sides {
            match line.first() {
                Some(b'-') => removed.text.extend_from_slice(&line[1..]),
                Some(b'+') => added.text.extend_from_slice(&line[1..]),
                _ => {}
            }
        }
    }
    if let Some((removed, added)) = sides {
        if !removed.text.is_empty() {
            hunks.push(removed);
        }
        if !added.text.is_empty() {
            hunks.push(added);
        }
    }
    Ok(())
}
