//! Conservative changed-line selection, independent of scope and aggregation.
use std::collections::HashSet;

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
