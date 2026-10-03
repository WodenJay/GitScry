//! Read path/status signals without reading file contents or concatenating patches.
use std::path::PathBuf;

use super::Repository;
use crate::app::AppError;

pub(crate) struct CurrentChange {
    pub(crate) staged: bool,
    pub(crate) head: Option<String>,
    pub(crate) changes: Vec<CurrentPath>,
    pub(crate) content: Vec<CurrentHunk>,
    pub(crate) content_omissions: Vec<ContentOmission>,
}

pub(crate) struct CurrentPath {
    pub(crate) status: String,
    pub(crate) old_path: Option<Vec<u8>>,
    pub(crate) new_path: Option<Vec<u8>>,
}

#[derive(Clone)]
pub(crate) struct CurrentHunk {
    pub(crate) path: Vec<u8>,
    pub(crate) added: bool,
    pub(crate) old_start: usize,
    pub(crate) new_start: usize,
    pub(crate) text: Vec<u8>,
}

pub(crate) struct ContentOmission {
    pub(crate) path: Vec<u8>,
    pub(crate) reason: &'static str,
}
impl CurrentChange {
    pub(crate) fn paths(&self) -> Vec<Vec<u8>> {
        let mut paths = self
            .changes
            .iter()
            .flat_map(|change| change.old_path.iter().chain(&change.new_path).cloned())
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        paths
    }
}

impl Repository {
    pub(crate) fn current_change(&self, staged: bool) -> Result<CurrentChange, AppError> {
        if !self
            .git
            .output(["ls-files", "--unmerged", "-z"], &[])?
            .is_empty()
        {
            return Err(AppError::input(
                "unresolved conflicts; resolve the index before running context",
            ));
        }
        let head = self
            .git
            .success(["rev-parse", "--verify", "--quiet", "HEAD"])?
            .then(|| {
                self.git
                    .text(["rev-parse", "HEAD"])
                    .map(|head| head.trim().to_owned())
            })
            .transpose()?;
        // Compare HEAD with the actual worktree even when a path was removed
        // from the real index. Merge HEAD into a private index copy to retain
        // sparse-checkout flags without modifying the caller's index.
        let temporary = if !staged && head.is_some() {
            Some(tempfile::tempdir().map_err(|error| {
                AppError::operational(format!("error: creating temporary change index: {error}"))
            })?)
        } else {
            None
        };
        let git = if let Some(temporary) = &temporary {
            let index = temporary.path().join("index");
            let source_index = PathBuf::from(
                self.git
                    .text(["rev-parse", "--path-format=absolute", "--git-path", "index"])?
                    .trim(),
            );
            let git = self.git.with_index(index.clone());
            match std::fs::copy(source_index, &index) {
                Ok(_) => {
                    git.output(
                        [
                            "-c",
                            "core.splitIndex=false",
                            "read-tree",
                            "-m",
                            "-i",
                            "HEAD",
                        ],
                        &[],
                    )?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    git.output(["-c", "core.splitIndex=false", "read-tree", "HEAD"], &[])?;
                }
                Err(error) => {
                    return Err(AppError::operational(format!(
                        "error: copying temporary change index: {error}"
                    )));
                }
            }
            git
        } else {
            self.git.clone()
        };
        let mut args = vec![
            "diff",
            "--name-status",
            "-z",
            "--find-renames",
            "--no-ext-diff",
            "--no-textconv",
            "--ignore-submodules=dirty",
        ];
        if staged {
            args.push("--cached");
        }
        let mut changes = if head.is_some() || staged {
            if head.is_some() {
                args.push("HEAD");
            }
            args.push("--");
            parse_paths(&git.output(args, &[])?)?
        } else {
            // An unborn worktree has no baseline: tracked files still present are additions.
            self.git
                .output(["ls-files", "--cached", "-z"], &[])?
                .split(|byte| *byte == 0)
                .filter(|path| !path.is_empty() && path_exists(&self.root, path))
                .map(|path| CurrentPath {
                    status: "A".to_owned(),
                    old_path: None,
                    new_path: Some(path.to_vec()),
                })
                .collect()
        };
        if !staged {
            for path in git
                .output(["ls-files", "--others", "--exclude-standard", "-z"], &[])?
                .split(|byte| *byte == 0)
            {
                if !path.is_empty() {
                    changes.push(CurrentPath {
                        status: "untracked".to_owned(),
                        old_path: None,
                        new_path: Some(path.to_vec()),
                    });
                }
            }
        }
        changes.sort_by(|a, b| {
            a.new_path
                .as_ref()
                .or(a.old_path.as_ref())
                .cmp(&b.new_path.as_ref().or(b.old_path.as_ref()))
                .then_with(|| a.status.cmp(&b.status))
        });
        let (content, content_omissions) =
            super::current_content::read(&git, &self.root, &changes, staged, head.is_some())?;
        Ok(CurrentChange {
            staged,
            head,
            changes,
            content,
            content_omissions,
        })
    }
}

fn parse_paths(bytes: &[u8]) -> Result<Vec<CurrentPath>, AppError> {
    let mut fields = bytes
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    let mut changes = Vec::new();
    while let Some(status) = fields.next() {
        let status = std::str::from_utf8(status)
            .map_err(|_| invalid_output())?
            .to_owned();
        let first = fields.next().ok_or_else(invalid_output)?.to_vec();
        let (old_path, new_path) = match status.as_bytes().first() {
            Some(b'R' | b'C') => (
                Some(first),
                Some(fields.next().ok_or_else(invalid_output)?.to_vec()),
            ),
            Some(b'D') => (Some(first), None),
            Some(b'A') => (None, Some(first)),
            Some(b'M' | b'T') => (Some(first.clone()), Some(first)),
            _ => return Err(invalid_output()),
        };
        changes.push(CurrentPath {
            status,
            old_path,
            new_path,
        });
    }
    Ok(changes)
}

fn invalid_output() -> AppError {
    AppError::operational("error: invalid current-change Git output")
}

pub(crate) fn worktree_path(root: &std::path::Path, path: &[u8]) -> std::path::PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        root.join(std::ffi::OsStr::from_bytes(path))
    }
    #[cfg(not(unix))]
    {
        root.join(String::from_utf8_lossy(path).as_ref())
    }
}

fn path_exists(root: &std::path::Path, path: &[u8]) -> bool {
    std::fs::symlink_metadata(worktree_path(root, path)).is_ok()
}

/// Reject every symlink component before checking a current test file. Never follow
/// historical traversal paths or external symlinks supplied through the cache.
pub(crate) fn current_regular_file(root: &std::path::Path, path: &[u8]) -> bool {
    let full = worktree_path(root, path);
    let Ok(relative) = full.strip_prefix(root) else {
        return false;
    };
    let mut current = root.to_path_buf();
    let mut final_is_file = false;
    for component in relative.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return false;
        }
        current.push(component);
        let Ok(metadata) = std::fs::symlink_metadata(&current) else {
            return false;
        };
        if metadata.file_type().is_symlink() {
            return false;
        }
        final_is_file = metadata.is_file();
    }
    final_is_file
}
