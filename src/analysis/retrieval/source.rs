use std::collections::{HashMap, HashSet};

use rusqlite::Connection;

use crate::{
    app::AppError,
    git::{Change, Snapshot},
};

use super::history::{self, HistoryCommit, HistoryHunk, PathChange};

pub(in crate::analysis) struct HistorySource<'a> {
    adapter: Adapter<'a>,
}

enum Adapter<'a> {
    Cached(&'a Connection),
    Direct(&'a Snapshot),
}

impl<'a> HistorySource<'a> {
    pub(in crate::analysis) fn cached(connection: &'a Connection) -> Self {
        Self {
            adapter: Adapter::Cached(connection),
        }
    }

    pub(in crate::analysis) fn direct(snapshot: &'a Snapshot) -> Self {
        Self {
            adapter: Adapter::Direct(snapshot),
        }
    }

    pub(in crate::analysis) fn path(
        &self,
        path: &[u8],
        reachable: &HashSet<String>,
    ) -> Result<Vec<HistoryCommit>, AppError> {
        match self.adapter {
            Adapter::Cached(connection) => history::path_history(connection, path, reachable),
            Adapter::Direct(snapshot) => Ok(direct_path_history(snapshot, path, reachable)),
        }
    }

    pub(in crate::analysis) fn hunks(&self, oid: &str) -> Result<Vec<HistoryHunk>, AppError> {
        match self.adapter {
            Adapter::Cached(connection) => history::hunks(connection, oid),
            Adapter::Direct(snapshot) => Ok(snapshot
                .hunks
                .iter()
                .filter(|hunk| hunk.commit_oid == oid)
                .map(|hunk| HistoryHunk {
                    change_ordinal: hunk.change_ordinal,
                    old_start: hunk.old_start,
                    old_lines: hunk.old_lines,
                    new_start: hunk.new_start,
                    new_lines: hunk.new_lines,
                    text: hunk.text.clone(),
                })
                .collect()),
        }
    }

    pub(in crate::analysis) fn has_missing_objects(
        &self,
        commits: &[HistoryCommit],
    ) -> Result<bool, AppError> {
        match self.adapter {
            Adapter::Cached(connection) => history::has_missing_objects(connection, commits),
            Adapter::Direct(snapshot) => {
                let missing = snapshot.missing_objects.iter().collect::<HashSet<_>>();
                Ok(commits.iter().any(|commit| {
                    commit
                        .changes
                        .iter()
                        .filter(|change| commit.anchored_ordinals.contains(&change.ordinal))
                        .flat_map(|change| [&change.old_blob, &change.new_blob])
                        .flatten()
                        .any(|blob| missing.contains(blob))
                }))
            }
        }
    }
}

fn direct_path_history(
    snapshot: &Snapshot,
    path: &[u8],
    reachable: &HashSet<String>,
) -> Vec<HistoryCommit> {
    let mut changes_by_path = HashMap::<Vec<u8>, Vec<&Change>>::new();
    let mut changes_by_commit = HashMap::<String, Vec<&Change>>::new();
    for change in &snapshot.changes {
        if !reachable.contains(&change.commit_oid) {
            continue;
        }
        for changed_path in [&change.old_path, &change.new_path].into_iter().flatten() {
            changes_by_path
                .entry(changed_path.clone())
                .or_default()
                .push(change);
        }
        changes_by_commit
            .entry(change.commit_oid.clone())
            .or_default()
            .push(change);
    }

    let mut pending = vec![path.to_vec()];
    let mut visited_paths = HashSet::new();
    let mut anchored = HashMap::<String, HashSet<i64>>::new();
    while let Some(current) = pending.pop() {
        if !visited_paths.insert(current.clone()) {
            continue;
        }
        for change in changes_by_path.get(&current).into_iter().flatten() {
            anchored
                .entry(change.commit_oid.clone())
                .or_default()
                .insert(change.ordinal);
            if (change.status.starts_with('R') || change.status.starts_with('C'))
                && change.new_path.as_deref() == Some(current.as_slice())
                && let Some(old_path) = &change.old_path
            {
                pending.push(old_path.clone());
            } else if (change.status.starts_with('R') || change.status.starts_with('C'))
                && change.old_path.as_deref() == Some(current.as_slice())
                && let Some(new_path) = &change.new_path
            {
                pending.push(new_path.clone());
            }
        }
    }

    let positions = snapshot
        .commits
        .iter()
        .enumerate()
        .map(|(position, commit)| (commit.oid.as_str(), position as i64))
        .collect::<HashMap<_, _>>();
    let commits = snapshot
        .commits
        .iter()
        .map(|commit| (commit.oid.as_str(), commit))
        .collect::<HashMap<_, _>>();
    let mut result = anchored
        .into_iter()
        .filter_map(|(oid, anchored_ordinals)| {
            let commit = commits.get(oid.as_str())?;
            let changes = changes_by_commit.get(&oid)?;
            let (subject, body) = super::message_parts(&commit.message);
            let mut paths = Vec::new();
            let mut all_changes = Vec::new();
            for change in changes {
                for changed_path in [&change.old_path, &change.new_path].into_iter().flatten() {
                    if !paths.contains(changed_path) {
                        paths.push(changed_path.clone());
                    }
                }
                all_changes.push(PathChange {
                    ordinal: change.ordinal,
                    status: change.status.clone(),
                    old_path: change.old_path.clone(),
                    new_path: change.new_path.clone(),
                    old_blob: change.old_blob.clone(),
                    new_blob: change.new_blob.clone(),
                });
            }
            all_changes.sort_by_key(|change| change.ordinal);
            Some(HistoryCommit {
                position: *positions.get(oid.as_str())?,
                oid,
                commit_time: commit.time,
                subject,
                body,
                paths,
                changes: all_changes,
                anchored_ordinals: anchored_ordinals.into_iter().collect(),
                parent_count: commit.parents.len(),
            })
        })
        .collect::<Vec<_>>();
    result.sort_by(|left, right| {
        right
            .position
            .cmp(&left.position)
            .then_with(|| left.oid.cmp(&right.oid))
    });
    result
}
