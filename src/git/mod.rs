mod conflicts;
pub(crate) use conflicts::MergeConflict;
mod current;
mod current_content;
pub(crate) use current::CurrentHunk;
pub(crate) use current::{CurrentChange, current_regular_file};
mod history;
mod process;
mod symbol;
mod symbol_history;
mod target;

use std::{collections::HashMap, path::PathBuf};

use crate::app::{AppError, IndexStage};

pub(crate) use history::{Change, Commit, HistoryTarget, Hunk, PatchStream, Snapshot};
use process::Git;
pub(crate) use target::TimelineTarget;
pub(crate) use target::{DeletedLine, RegressionTarget, TraceFixTarget, WhyAnchor, WhyTarget};

pub(crate) struct SymbolTrace {
    pub(crate) revisions: Vec<String>,
    pub(crate) modifications: Vec<SymbolChange>,
    pub(crate) paths: Vec<Vec<u8>>,
    pub(crate) introduction: Result<String, String>,
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
    pub(crate) fn merge_conflict(&self) -> Result<MergeConflict, AppError> {
        conflicts::pin(self)
    }
    pub(crate) fn tracked_files(&self, revision: &str) -> Result<Vec<Vec<u8>>, AppError> {
        let tree = self.git.output(["ls-tree", "-r", "-z", revision], &[])?;
        let mut paths = Vec::new();
        for entry in tree
            .split(|byte| *byte == 0)
            .filter(|entry| !entry.is_empty())
        {
            let Some(tab) = entry.iter().position(|byte| *byte == b'\t') else {
                return Err(AppError::operational("error: invalid Git tree entry"));
            };
            if entry[..tab].split(|byte| *byte == b' ').nth(1) == Some(b"blob") {
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
        if matches!(&target.anchor, WhyAnchor::Symbol { .. }) {
            target.symbol_trace = Some(self.trace_why_symbol(&target));
        }
        Ok(target)
    }

    pub(crate) fn trace_why_symbol(&self, target: &WhyTarget) -> SymbolTrace {
        symbol_history::trace(&self.git, target)
    }

    pub(crate) fn pin_timeline_target(
        &self,
        revision: &str,
        path: &str,
    ) -> Result<TimelineTarget, AppError> {
        target::pin_timeline(&self.git, revision, path)
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
        target::pin_regression(&self.git, bad_revision, good_revision, path, symbol)
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
