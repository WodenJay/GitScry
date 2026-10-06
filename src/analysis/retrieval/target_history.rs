//! Conservative changed-line selection, independent of scope and aggregation.
use std::collections::{HashMap, HashSet};

use crate::{
    app::AppError,
    cache::QuerySession,
    git::{WhyAnchor, WhyTarget},
};

pub(in crate::analysis) struct Selection {
    pub(in crate::analysis) touches: HashSet<String>,
    pub(in crate::analysis) paths: Vec<Vec<u8>>,
    pub(in crate::analysis) complete: bool,
    pub(in crate::analysis) limitations: Vec<String>,
}

pub(in crate::analysis) fn select(
    session: &QuerySession,
    target: &WhyTarget,
    shallow_boundaries: &[String],
) -> Result<Selection, AppError> {
    match &target.anchor {
        WhyAnchor::Line { .. } => select_line(session, target, shallow_boundaries),
        WhyAnchor::Symbol { .. } => select_symbol(session, target, shallow_boundaries),
    }
}

fn select_line(
    session: &QuerySession,
    target: &WhyTarget,
    shallow_boundaries: &[String],
) -> Result<Selection, AppError> {
    let reachable = session.first_parent_ancestors(&target.revision)?;
    let commits = session.path_history(&target.path, &reachable)?;
    let WhyAnchor::Line { number } = target.anchor else {
        unreachable!()
    };
    let mut line = number as i64;
    let mut path = target.path.clone();
    let mut blob = None;
    let mut selection = Selection {
        touches: HashSet::new(), paths: vec![path.clone()], complete: false,
        limitations: vec!["Changed-line history follows first parents only; merge replay and commits changing more than 50 paths are excluded from relation statistics. Independent file-level follow-on material is omitted.".to_owned()],
    };
    for commit in commits {
        let changes = commit
            .changes
            .iter()
            .filter(|change| change.new_path.as_ref() == Some(&path))
            .collect::<Vec<_>>();
        let [change] = changes.as_slice() else {
            continue;
        };
        if commit.parent_count > 1
            || commit.shallow_boundary
            || shallow_boundaries.contains(&commit.oid)
        {
            selection.limitations.push("Tracing stopped at a merge or shallow-history boundary; earlier target touches are indeterminate.".to_owned());
            return Ok(selection);
        }
        if blob
            .as_ref()
            .is_some_and(|blob| change.new_blob.as_ref() != Some(blob))
            || session.has_missing_objects(std::slice::from_ref(&commit))?
        {
            selection.limitations.push(
                "Tracing stopped at missing objects or discontinuous file history.".to_owned(),
            );
            return Ok(selection);
        }
        let hunks = session
            .history_hunks(&commit.oid)?
            .into_iter()
            .filter(|hunk| hunk.change_ordinal == change.ordinal)
            .collect::<Vec<_>>();
        let Some((changed, parent_line, introduced)) =
            super::line_history::map_line(&hunks, line, change.old_blob == change.new_blob)
        else {
            selection.limitations.push(
                "Tracing stopped because complete changed-line patch material is unavailable."
                    .to_owned(),
            );
            return Ok(selection);
        };
        if changed.is_some() {
            selection.touches.insert(commit.oid.clone());
        }
        if change.status.starts_with('A') || introduced {
            selection.complete = true;
            return Ok(selection);
        }
        if change.status.starts_with('C') {
            selection.limitations.push(
                "Tracing stopped at a copy boundary; source-file history is not target history."
                    .to_owned(),
            );
            return Ok(selection);
        }
        line = parent_line;
        blob = change.old_blob.clone();
        if let Some(old_path) = &change.old_path
            && old_path != &path
        {
            selection.limitations.push("Target history follows cached rename identity; ambiguous or unavailable rename history cannot establish earlier touches.".to_owned());
            path = old_path.clone();
            if !selection.paths.contains(&path) {
                selection.paths.push(path.clone());
            }
        }
    }
    selection.limitations.push("Published history did not establish the target's introduction; earlier target touches are indeterminate.".to_owned());
    Ok(selection)
}

/// Shared confirmation facts; command scope and path-move policy stay with callers.
pub(in crate::analysis) enum SymbolHistoryLimitation<'a> {
    Trace(&'a str),
    CacheGap,
    MissingObjects,
    UncertainLineage,
    IntroductionAbsent,
}

pub(in crate::analysis) fn confirm_symbol_introduction<'a>(
    trace: &'a crate::git::SymbolTrace,
    commits: &'a [crate::cache::HistoryCommit],
    reachable: &HashSet<String>,
    shallow: bool,
    missing_objects: bool,
    mut required_revisions: impl Iterator<Item = &'a String>,
) -> Result<&'a crate::cache::HistoryCommit, SymbolHistoryLimitation<'a>> {
    let oid = trace
        .introduction
        .as_ref()
        .map_err(|reason| SymbolHistoryLimitation::Trace(reason))?;
    let history = commits
        .iter()
        .map(|commit| (commit.oid.as_str(), commit))
        .collect::<HashMap<_, _>>();
    if required_revisions.any(|oid| !reachable.contains(oid) || !history.contains_key(oid.as_str()))
        || !reachable.contains(oid)
    {
        return Err(SymbolHistoryLimitation::CacheGap);
    }
    if missing_objects {
        return Err(SymbolHistoryLimitation::MissingObjects);
    }
    if shallow
        || trace.revisions.iter().any(|oid| {
            history
                .get(oid.as_str())
                .is_some_and(|commit| commit.parent_count > 1 || commit.shallow_boundary)
        })
    {
        return Err(SymbolHistoryLimitation::UncertainLineage);
    }
    history
        .get(oid.as_str())
        .copied()
        .ok_or(SymbolHistoryLimitation::IntroductionAbsent)
}

fn select_symbol(
    session: &QuerySession,
    target: &WhyTarget,
    shallow_boundaries: &[String],
) -> Result<Selection, AppError> {
    let Some(trace) = target.symbol_trace.as_ref() else {
        return Err(AppError::operational(
            "symbol target is missing its history trace",
        ));
    };
    let reachable = session.first_parent_ancestors(&target.revision)?;
    let mut paths = vec![target.path.clone()];
    for path in &trace.paths {
        if !paths.contains(path) {
            paths.push(path.clone());
        }
    }
    let path_moved = trace.paths.iter().any(|path| path != &target.path);
    let mut commits_by_oid = HashMap::new();
    for path in &paths {
        for commit in session.path_history(path, &reachable)? {
            commits_by_oid.entry(commit.oid.clone()).or_insert(commit);
        }
    }
    let commits = commits_by_oid.into_values().collect::<Vec<_>>();
    let confirmation = confirm_symbol_introduction(
        trace,
        &commits,
        &reachable,
        target.shallow
            || trace
                .revisions
                .iter()
                .any(|oid| shallow_boundaries.contains(oid)),
        session.has_missing_objects(&commits)?,
        trace
            .revisions
            .iter()
            .chain(trace.modifications.iter().map(|change| &change.oid)),
    );
    let mut selection = Selection {
        touches: trace
            .modifications
            .iter()
            .filter(|change| !shallow_boundaries.contains(&change.oid))
            .map(|change| change.oid.clone())
            .collect(),
        paths,
        complete: false,
        limitations: vec![
            "Symbol history follows first parents only; merge replay and commits changing more than 50 paths are excluded from relation statistics. Independent file-level follow-on material is omitted."
                .to_owned(),
        ],
    };
    if path_moved {
        selection.limitations.push(
            "Symbol history crossed a file path move; earlier target touches may be incomplete."
                .to_owned(),
        );
    }
    match confirmation {
        Err(SymbolHistoryLimitation::Trace(reason)) => selection.limitations.push(format!(
            "Symbol introduction could not be confirmed: {reason}."
        )),
        Err(SymbolHistoryLimitation::CacheGap) => selection.limitations.push(
            "Symbol history extends beyond published cache coverage; earlier target touches are indeterminate."
                .to_owned(),
        ),
        Err(SymbolHistoryLimitation::MissingObjects) => selection.limitations.push(
            "Local Git objects needed to confirm the symbol introduction are missing."
                .to_owned(),
        ),
        Err(SymbolHistoryLimitation::UncertainLineage) => selection.limitations.push(
            "Shallow or merge history makes symbol lineage uncertain."
                .to_owned(),
        ),
        Err(SymbolHistoryLimitation::IntroductionAbsent) => selection.limitations.push(
            "Symbol introduction is not present in cached path history.".to_owned(),
        ),
        Ok(commit) => {
            selection.touches.insert(commit.oid.clone());
            selection.complete = !path_moved;
        }
    }
    Ok(selection)
}
