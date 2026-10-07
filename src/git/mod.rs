mod conflicts;
pub(crate) use conflicts::{ConflictFile, MergeConflict};
mod current;
mod current_content;
pub(crate) use current::CurrentHunk;
pub(crate) use current::{CurrentChange, current_regular_file};
mod history;
mod merge_tree;
pub(crate) use merge_tree::{MergeTree, Replay, StageEntry, TreeEntry};
mod patch;
mod process;
mod symbol;
mod symbol_history;
mod target;

use std::{collections::HashMap, path::PathBuf};

use crate::app::{AppError, IndexStage};

pub(crate) use history::{Change, Commit, HistoryTarget, Hunk, PatchStream, Snapshot};
pub(crate) use patch::{PatchLookup, PatchSpec};
use process::Git;

/// Canonicalize repository-relative paths like Git tree paths.
pub(crate) fn normalize_git_path(path: &[u8]) -> Vec<u8> {
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

pub(crate) use target::FateTarget;
pub(crate) use target::TimelineTarget;
pub(crate) use target::{DeletedLine, RegressionTarget, TraceFixTarget, WhyAnchor, WhyTarget};

/// A located historical declaration: inclusive line span plus structured selection.
pub(crate) struct SymbolLocation {
    pub(crate) start_line: usize,
    pub(crate) end_line: usize,
    pub(crate) body_span: Option<symbol::BodySpan>,
    pub(crate) selection: SymbolSelection,
}
pub(crate) use symbol::BodySpan as SymbolBodySpan;
pub(crate) use symbol::Selection as SymbolSelection;
pub(crate) struct SymbolTrace {
    pub(crate) revisions: Vec<String>,
    pub(crate) modifications: Vec<SymbolChange>,
    pub(crate) paths: Vec<Vec<u8>>,
    pub(crate) introduction: Result<String, String>,
    pub(crate) warnings: Vec<String>,
}

pub(crate) struct SymbolChange {
    pub(crate) oid: String,
    pub(crate) path: Vec<u8>,
    pub(crate) start: usize,
    pub(crate) end: usize,
}
pub(crate) struct Repository {
    pub(crate) root: PathBuf,
    pub(crate) common_dir: PathBuf,
    git: Git,
}

impl Repository {
    pub(crate) fn complete_patch(&self, oid: &str) -> Result<history::PatchData, AppError> {
        history::read_complete_patch(&self.git, oid)
    }

    pub(crate) fn patch_tree_objects(
        &self,
        oid: &str,
        parent: Option<&str>,
    ) -> Result<Vec<String>, AppError> {
        let mut args = vec!["rev-parse".to_owned(), format!("{oid}^{{tree}}")];
        args.extend(parent.map(|parent| format!("{parent}^{{tree}}")));
        Ok(self.git.text(args)?.lines().map(str::to_owned).collect())
    }

    /// Only locally available branch refs; never contacts a remote.
    pub(crate) fn patch_branch_tips(&self) -> Result<Vec<(String, String)>, AppError> {
        let refs = self.git.text([
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads",
            "refs/remotes",
        ])?;
        Ok(refs
            .lines()
            .filter_map(|line| {
                line.split_once(' ')
                    .map(|(name, oid)| (name.to_owned(), oid.to_owned()))
            })
            .collect())
    }
    pub(crate) fn merge_conflict(&self) -> Result<MergeConflict, AppError> {
        conflicts::pin(self)
    }
    pub(crate) fn tracked_files(&self, revision: &str) -> Result<Vec<Vec<u8>>, AppError> {
        self.tree_paths(revision, false)
    }
    pub(crate) fn regular_files(&self, revision: &str) -> Result<Vec<Vec<u8>>, AppError> {
        self.tree_paths(revision, true)
    }
    fn tree_paths(&self, revision: &str, regular_only: bool) -> Result<Vec<Vec<u8>>, AppError> {
        let tree = self.git.output(["ls-tree", "-r", "-z", revision], &[])?;
        let mut paths = Vec::new();
        for entry in tree
            .split(|byte| *byte == 0)
            .filter(|entry| !entry.is_empty())
        {
            let Some(tab) = entry.iter().position(|byte| *byte == b'\t') else {
                return Err(AppError::operational("error: invalid Git tree entry"));
            };
            let mut header = entry[..tab].split(|byte| *byte == b' ');
            let mode = header.next();
            let kind = header.next();
            let regular_mode = mode.is_some_and(|mode| mode == b"100644" || mode == b"100755");
            if kind == Some(&b"blob"[..]) && (!regular_only || regular_mode) {
                paths.push(entry[tab + 1..].to_vec());
            }
        }
        Ok(paths)
    }
    pub(crate) fn discover() -> Result<Self, AppError> {
        let cwd = std::env::current_dir().map_err(|error| {
            AppError::operational(format!("error: reading current directory: {error}"))
        })?;
        let probe = Git::new(cwd);
        let bare = probe.text(["rev-parse", "--is-bare-repository"])?;
        if bare.trim() == "true" {
            return Err(AppError::operational(
                "error: bare repositories are not supported; run GitScry in a worktree",
            ));
        }
        let root = PathBuf::from(probe.text(["rev-parse", "--show-toplevel"])?.trim());
        let common_dir = PathBuf::from(
            probe
                .text(["rev-parse", "--path-format=absolute", "--git-common-dir"])?
                .trim(),
        );
        Ok(Self {
            git: Git::new(root.clone()),
            root,
            common_dir,
        })
    }

    pub(crate) fn pin_why_target(
        &self,
        revision: &str,
        path: &str,
        anchor: WhyAnchor,
    ) -> Result<WhyTarget, AppError> {
        let mut target = target::pin(&self.git, Some(revision), path, anchor)?;
        if let Some(selection) = &target.symbol_selection {
            target.symbol_trace = Some(self.prepare_symbol_trace(
                &target.revision,
                &target.path,
                selection,
                &mut target.warnings,
            ));
        }
        Ok(target)
    }

    fn prepare_symbol_trace(
        &self,
        revision: &str,
        path: &[u8],
        selection: &SymbolSelection,
        warnings: &mut Vec<String>,
    ) -> SymbolTrace {
        let trace = symbol_history::trace(&self.git, revision, path, selection);
        for warning in &trace.warnings {
            if !warnings.contains(warning) {
                warnings.push(warning.clone());
            }
        }
        trace
    }

    pub(crate) fn pin_timeline_target(
        &self,
        revision: &str,
        path: &str,
    ) -> Result<TimelineTarget, AppError> {
        target::pin_timeline(&self.git, revision, path)
    }

    pub(crate) fn pin_fate_target(
        &self,
        revision: &str,
        path: &str,
        line: Option<usize>,
        symbol: Option<&str>,
    ) -> Result<FateTarget, AppError> {
        target::pin_fate(&self.git, revision, path, line, symbol)
    }

    /// Read a blob's contents at a revision; `None` when the path is absent or not a blob.
    pub(crate) fn read_blob(
        &self,
        revision: &str,
        path: &[u8],
    ) -> Result<Option<Vec<u8>>, AppError> {
        let Ok(path) = std::str::from_utf8(path) else {
            return Ok(None);
        };
        target::read_blob_at(&self.git, revision, path)
    }

    /// Resolve a symbol declaration uniquely inside one source version.
    pub(crate) fn locate_symbol(
        &self,
        content: &[u8],
        name: &str,
        path: &str,
    ) -> Result<SymbolLocation, AppError> {
        let location = symbol::locate_unique(content, name, path)?;
        Ok(SymbolLocation {
            start_line: location.span.start,
            end_line: location.span.end,
            body_span: location.body_span,
            selection: location.selection,
        })
    }

    pub(crate) fn file_paths_at(
        &self,
        revision: &str,
    ) -> Result<std::collections::BTreeSet<Vec<u8>>, AppError> {
        let output = self
            .git
            .output(["ls-tree", "-r", "--name-only", "-z", revision], &[])?;
        Ok(output
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| path.to_vec())
            .collect())
    }

    pub(crate) fn resolve_commit(&self, revision: &str) -> Result<String, AppError> {
        if revision.contains('\0') {
            return Err(AppError::input("revision must not contain a NUL byte"));
        }
        target::resolve_revision(&self.git, revision)
    }

    pub(crate) fn pin_regression_target(
        &self,
        bad_revision: &str,
        good_revision: Option<&str>,
        path: &str,
        symbol: Option<&str>,
    ) -> Result<RegressionTarget, AppError> {
        let mut target =
            target::pin_regression(&self.git, bad_revision, good_revision, path, symbol)?;
        if let Some(selection) = &target.symbol_selection {
            target.symbol_trace = Some(self.prepare_symbol_trace(
                &target.bad_revision,
                &target.path,
                selection,
                &mut target.warnings,
            ));
        }
        Ok(target)
    }

    pub(crate) fn pin_trace_fix(
        &self,
        revision: &str,
        paths: &[String],
    ) -> Result<TraceFixTarget, AppError> {
        target::pin_trace_fix(&self.git, revision, paths)
    }

    pub(crate) fn object_format(&self) -> Result<String, AppError> {
        Ok(self
            .git
            .text(["rev-parse", "--show-object-format"])? // pinned before cache preparation
            .trim()
            .to_owned())
    }

    pub(crate) fn shallow_boundaries(&self) -> Result<Vec<String>, AppError> {
        let mut boundaries = history::read_shallow_boundaries(&self.git)?;
        boundaries.sort();
        boundaries.dedup();
        Ok(boundaries)
    }

    pub(crate) fn missing_objects(&self, object_ids: &[String]) -> Result<Vec<String>, AppError> {
        self.git.missing_objects(object_ids)
    }

    pub(crate) fn reachable_commits(&self, tip: &str) -> Result<Vec<String>, AppError> {
        let output = self.git.text(["rev-list", tip])?;
        Self::parse_reachable_commits(&output)
    }

    /// Commit union reachable from all tips, newest-first (rev-list default order).
    pub(crate) fn reachable_commits_from_tips(
        &self,
        tips: &[String],
    ) -> Result<Vec<String>, AppError> {
        let output = self.git.text(
            ["rev-list"]
                .into_iter()
                .chain(tips.iter().map(String::as_str)),
        )?;
        Self::parse_reachable_commits(&output)
    }

    pub(crate) fn reachable_commits_in_history_order(
        &self,
        tip: &str,
    ) -> Result<Vec<String>, AppError> {
        let output = self
            .git
            .text(["rev-list", "--reverse", "--topo-order", tip])?;
        Self::parse_reachable_commits(&output)
    }

    fn parse_reachable_commits(output: &str) -> Result<Vec<String>, AppError> {
        output
            .lines()
            .map(|oid| {
                if matches!(oid.len(), 40 | 64) && oid.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    Ok(oid.to_owned())
                } else {
                    Err(AppError::operational(
                        "error: Git revision graph contained an invalid object ID",
                    ))
                }
            })
            .collect()
    }

    pub(crate) fn reachable_commit_parents(
        &self,
        tip: &str,
    ) -> Result<HashMap<String, Vec<String>>, AppError> {
        let output = self.git.text(["rev-list", "--parents", tip])?;
        let parse_oid = |oid: &str| {
            if matches!(oid.len(), 40 | 64) && oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                Ok(oid.to_owned())
            } else {
                Err(AppError::operational(
                    "error: Git revision graph contained an invalid object ID",
                ))
            }
        };
        let mut graph = HashMap::new();
        for line in output.lines() {
            let mut fields = line.split_ascii_whitespace();
            let oid = parse_oid(fields.next().ok_or_else(|| {
                AppError::operational("error: Git revision graph contained an invalid object ID")
            })?)?;
            let parents = fields.map(parse_oid).collect::<Result<Vec<_>, _>>()?;
            graph.insert(oid, parents);
        }
        Ok(graph)
    }

    /// Return a commit's raw parents, including parents hidden by shallow history.
    pub(crate) fn commit_parents(&self, oid: &str) -> Result<Vec<String>, AppError> {
        let output = self.git.output(["cat-file", "commit", oid], &[])?;
        let parse_oid = |object_id: &[u8]| {
            let object_id = std::str::from_utf8(object_id).map_err(|_| {
                AppError::operational("error: Git commit object contained an invalid object ID")
            })?;
            if matches!(object_id.len(), 40 | 64)
                && object_id.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                Ok(object_id.to_owned())
            } else {
                Err(AppError::operational(
                    "error: Git commit object contained an invalid object ID",
                ))
            }
        };
        let mut parents = Vec::new();
        let mut has_tree = false;
        let mut has_separator = false;
        for header in output.split(|byte| *byte == b'\n') {
            if header.is_empty() {
                has_separator = true;
                break;
            }
            if let Some(tree) = header.strip_prefix(b"tree ") {
                parse_oid(tree)?;
                has_tree = true;
            } else if let Some(parent) = header.strip_prefix(b"parent ") {
                parents.push(parse_oid(parent)?);
            }
        }
        if !has_tree || !has_separator {
            return Err(AppError::operational(
                "error: Git commit object contained invalid headers",
            ));
        }
        Ok(parents)
    }

    /// Whole-commit patch identifiers under the fixed diff rules in [`patch`].
    pub(crate) fn commit_patch_ids(
        &self,
        specs: &[PatchSpec],
    ) -> Result<Vec<(String, PatchLookup)>, AppError> {
        patch::commit_patch_ids(&self.git, specs)
    }

    pub(crate) fn reachable_commit_times(
        &self,
        tip: &str,
    ) -> Result<HashMap<String, i64>, AppError> {
        let output = self.git.text(["rev-list", "--timestamp", tip])?;
        let mut times = HashMap::new();
        for line in output.lines() {
            let mut fields = line.split_ascii_whitespace();
            let timestamp = fields.next().ok_or_else(|| {
                AppError::operational(
                    "error: Git revision graph contained an invalid commit timestamp",
                )
            })?;
            let oid = fields
                .next()
                .filter(|oid| {
                    matches!(oid.len(), 40 | 64) && oid.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
                .ok_or_else(|| {
                    AppError::operational(
                        "error: Git revision graph contained an invalid object ID",
                    )
                })?;
            if fields.next().is_some() {
                return Err(AppError::operational(
                    "error: Git revision graph contained an invalid commit timestamp",
                ));
            }
            let commit_time = timestamp.parse::<i64>().map_err(|_| {
                AppError::operational(
                    "error: Git revision graph contained an invalid commit timestamp",
                )
            })?;
            times.insert(oid.to_owned(), commit_time);
        }
        Ok(times)
    }
    pub(crate) fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool, AppError> {
        self.git
            .success(["merge-base", "--is-ancestor", ancestor, descendant])
    }

    pub(crate) fn read_history_at(
        &self,
        target: HistoryTarget,
        report: &mut dyn FnMut(IndexStage),
    ) -> Result<Snapshot, AppError> {
        history::read(&self.git, target, report)
    }

    pub(crate) fn read_incremental_history_at(
        &self,
        target: HistoryTarget,
        cached_commits: &[String],
        refresh_commits: &[String],
        known_missing_objects: Vec<String>,
        report: &mut dyn FnMut(IndexStage),
    ) -> Result<Snapshot, AppError> {
        history::read_incremental(
            &self.git,
            target,
            cached_commits,
            refresh_commits,
            known_missing_objects,
            report,
        )
    }
}
