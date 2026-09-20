mod history;
mod process;
mod target;

use std::path::PathBuf;

use crate::app::AppError;

pub(crate) use history::{Change, Commit, HistoryTarget, Hunk, Snapshot};
use process::Git;
pub(crate) use target::{DeletedLine, RegressionTarget, TraceFixTarget, WhyAnchor, WhyTarget};

pub(crate) struct Repository {
    pub(crate) root: PathBuf,
    git: Git,
}

impl Repository {
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
        Ok(Self {
            git: Git::new(root.clone()),
            root,
        })
    }

    pub(crate) fn pin_why_target(
        &self,
        revision: &str,
        path: &str,
        anchor: WhyAnchor,
    ) -> Result<WhyTarget, AppError> {
        target::pin(&self.git, Some(revision), path, anchor)
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

    pub(crate) fn default_target(&self) -> Result<(String, String), AppError> {
        let default_ref = self
            .resolve_default_branch()?
            .ok_or_else(|| default_branch_error("no default branch reference exists"))?;
        let tip = self.tip_for(&default_ref)?;
        Ok((default_ref, tip))
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
        self.git
            .text(["rev-list", tip])?
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

    pub(crate) fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool, AppError> {
        self.git
            .success(["merge-base", "--is-ancestor", ancestor, descendant])
    }

    pub(crate) fn read_default_history_at(
        &self,
        target: HistoryTarget,
    ) -> Result<Snapshot, AppError> {
        history::read(&self.git, target)
    }

    pub(crate) fn read_incremental_history_at(
        &self,
        target: HistoryTarget,
        cached_commits: &[String],
        refresh_commits: &[String],
        known_missing_objects: Vec<String>,
    ) -> Result<Snapshot, AppError> {
        history::read_incremental(
            &self.git,
            target,
            cached_commits,
            refresh_commits,
            known_missing_objects,
        )
    }

    fn tip_for(&self, default_ref: &str) -> Result<String, AppError> {
        let tip = self.git.text([
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{default_ref}^{{commit}}"),
        ])?;
        Ok(tip.trim().to_owned())
    }

    fn resolve_default_branch(&self) -> Result<Option<String>, AppError> {
        let refs = self.git.text([
            "for-each-ref",
            "--format=%(refname)%00%(symref)",
            "refs/remotes",
            "refs/heads/main",
            "refs/heads/master",
        ])?;
        let mut remote_heads = Vec::new();
        let mut local_defaults = Vec::new();
        let mut origin_head = None;

        for line in refs.lines() {
            let mut fields = line.split('\0');
            let name = fields.next().unwrap_or_default();
            let target = fields.next().unwrap_or_default();
            if name == "refs/remotes/origin/HEAD" && !target.is_empty() {
                origin_head = Some(target.to_owned());
            } else if name.starts_with("refs/remotes/")
                && name.ends_with("/HEAD")
                && !target.is_empty()
            {
                remote_heads.push(target.to_owned());
            } else if matches!(name, "refs/heads/main" | "refs/heads/master") {
                local_defaults.push(name.to_owned());
            }
        }

        if let Some(reference) = origin_head {
            return Ok(Some(reference));
        }
        remote_heads.sort();
        remote_heads.dedup();
        if remote_heads.len() == 1 {
            return Ok(Some(remote_heads.remove(0)));
        }
        if remote_heads.len() > 1 {
            return Err(default_branch_error("multiple remote HEADs"));
        }
        if local_defaults.len() == 1 {
            return Ok(Some(local_defaults.remove(0)));
        }
        if local_defaults.len() > 1 {
            return Err(default_branch_error("both main and master exist"));
        }
        Ok(None)
    }
}

fn default_branch_error(reason: &str) -> AppError {
    AppError::operational(format!(
        "error: resolving default branch: {reason}; configure origin/HEAD or keep exactly one local main/master branch"
    ))
}
