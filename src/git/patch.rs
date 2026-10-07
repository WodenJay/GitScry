//! Whole-commit patch identifiers for the propagation capability.
//!
//! A patch identifier is the `git patch-id --verbatim` hash of a commit's entire
//! diff against its first parent. The diff is generated with a fixed set of
//! options so ambient `diff.*`/`core.*` configuration, external diff drivers,
//! and text conversion cannot change the result, and whitespace is preserved.
//! Only whole-commit comparisons are supported; merge commits and commits with
//! no diff have no identifier.

use std::collections::HashMap;

use super::process::Git;
use crate::app::AppError;

/// Fixed diff generation. `--verbatim` keeps whitespace significant, while the
/// remaining options neutralise ambient configuration and external helpers.
const DIFF_ARGS: [&str; 17] = [
    "diff-tree",
    "--stdin",
    "--root",
    "-r",
    "-p",
    "--full-index",
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--no-renames",
    "--unified=3",
    "--diff-algorithm=myers",
    "--no-indent-heuristic",
    "--inter-hunk-context=0",
    "--ignore-submodules=none",
    "--src-prefix=a/",
    "--dst-prefix=b/",
];

/// One whole-commit patch to identify.
///
/// `first_parent` is `None` for a root commit, whose patch is generated against
/// the empty tree. Ordinary commits must pass their first parent; merge commits
/// are not supported and must be excluded by the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PatchSpec {
    pub(crate) oid: String,
    pub(crate) first_parent: Option<String>,
}

impl PatchSpec {
    pub(crate) fn new(oid: impl Into<String>, first_parent: Option<String>) -> Self {
        Self {
            oid: oid.into(),
            first_parent,
        }
    }
}

/// The outcome of looking up one commit's whole-commit patch identifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PatchLookup {
    /// The commit has a whole-commit patch with this identifier.
    Identifier(String),
    /// The commit has no patch to identify: its diff is empty.
    NoPatch,
    /// The commit's patch could not be read from the local object store.
    Unreadable,
}

/// Compute patch identifiers for the given specs, preserving input order.
///
/// When the local object store is missing objects for a commit, that commit is
/// reported as [`PatchLookup::Unreadable`] instead of failing the whole lookup.
pub(super) fn commit_patch_ids(
    git: &Git,
    specs: &[PatchSpec],
) -> Result<Vec<(String, PatchLookup)>, AppError> {
    if specs.is_empty() {
        return Ok(Vec::new());
    }
    let input = diff_input(specs);
    match git.output(DIFF_ARGS, input.as_bytes()) {
        Ok(diff) => {
            let identifiers = git.output(["patch-id", "--verbatim"], &diff)?;
            let by_commit = parse_identifiers(&identifiers)?;
            Ok(collect(specs, &by_commit))
        }
        Err(_) => {
            // A missing object aborts the batched diff. Fall back to one commit
            // at a time so readable commits still yield identifiers and only the
            // affected commit is reported unreadable.
            specs.iter().map(|spec| lookup_one(git, spec)).collect()
        }
    }
}

fn collect(specs: &[PatchSpec], by_commit: &HashMap<String, String>) -> Vec<(String, PatchLookup)> {
    specs
        .iter()
        .map(|spec| {
            let lookup = by_commit
                .get(&spec.oid)
                .map(|identifier| PatchLookup::Identifier(identifier.clone()))
                .unwrap_or(PatchLookup::NoPatch);
            (spec.oid.clone(), lookup)
        })
        .collect()
}

fn lookup_one(git: &Git, spec: &PatchSpec) -> Result<(String, PatchLookup), AppError> {
    let diff = match git.output(DIFF_ARGS, spec_input(spec).as_bytes()) {
        Ok(diff) => diff,
        Err(_) => return Ok((spec.oid.clone(), PatchLookup::Unreadable)),
    };
    let identifiers = git.output(["patch-id", "--verbatim"], &diff)?;
    let by_commit = parse_identifiers(&identifiers)?;
    let lookup = by_commit
        .get(&spec.oid)
        .map(|identifier| PatchLookup::Identifier(identifier.clone()))
        .unwrap_or(PatchLookup::NoPatch);
    Ok((spec.oid.clone(), lookup))
}

fn diff_input(specs: &[PatchSpec]) -> String {
    let mut input = String::new();
    for spec in specs {
        input.push_str(&spec_input(spec));
    }
    input
}

fn spec_input(spec: &PatchSpec) -> String {
    match &spec.first_parent {
        Some(parent) => format!("{} {parent}\n", spec.oid),
        None => format!("{}\n", spec.oid),
    }
}

fn parse_identifiers(output: &[u8]) -> Result<HashMap<String, String>, AppError> {
    let text = std::str::from_utf8(output).map_err(|error| {
        AppError::operational(format!(
            "error: parsing Git patch identifiers as UTF-8: {error}"
        ))
    })?;
    let mut by_commit = HashMap::new();
    for line in text.lines() {
        let mut fields = line.split_ascii_whitespace();
        let Some(identifier) = fields.next() else {
            continue;
        };
        let Some(commit) = fields.next() else {
            return Err(AppError::operational(
                "error: Git patch identifier output was malformed",
            ));
        };
        if fields.next().is_some() {
            return Err(AppError::operational(
                "error: Git patch identifier output was malformed",
            ));
        }
        by_commit.insert(commit.to_owned(), identifier.to_owned());
    }
    Ok(by_commit)
}
