use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, params};

use crate::app::AppError;

use super::super::search_error;
use super::text::message_parts;

pub(in crate::analysis) struct HistoryCommit {
    pub(in crate::analysis) position: i64,
    pub(in crate::analysis) oid: String,
    pub(in crate::analysis) commit_time: i64,
    pub(in crate::analysis) subject: String,
    pub(in crate::analysis) body: String,
    pub(in crate::analysis) paths: Vec<Vec<u8>>,
    pub(in crate::analysis) changes: Vec<PathChange>,
    pub(in crate::analysis) anchored_ordinals: Vec<i64>,
    pub(in crate::analysis) parent_count: usize,
}

pub(in crate::analysis) struct PathChange {
    pub(in crate::analysis) ordinal: i64,
    pub(in crate::analysis) status: String,
    pub(in crate::analysis) old_path: Option<Vec<u8>>,
    pub(in crate::analysis) new_path: Option<Vec<u8>>,
    pub(in crate::analysis) old_blob: Option<String>,
    pub(in crate::analysis) new_blob: Option<String>,
}

pub(in crate::analysis) struct HistoryHunk {
    pub(in crate::analysis) change_ordinal: i64,
    pub(in crate::analysis) old_start: i64,
    pub(in crate::analysis) old_lines: i64,
    pub(in crate::analysis) new_start: i64,
    pub(in crate::analysis) new_lines: i64,
    pub(in crate::analysis) text: Vec<u8>,
}

pub(in crate::analysis) fn path_history(
    connection: &Connection,
    path: &[u8],
    reachable: &HashSet<String>,
) -> Result<Vec<HistoryCommit>, AppError> {
    let mut pending = vec![path.to_vec()];
    let mut visited_paths = HashSet::new();
    let mut commits = HashMap::<String, HistoryCommit>::new();

    while let Some(path) = pending.pop() {
        if !visited_paths.insert(path.clone()) {
            continue;
        }
        let mut statement = connection
            .prepare(
                "SELECT c.rowid, c.oid, c.commit_time, c.message,
                        ch.ordinal, ch.status, ch.old_path, ch.new_path,
                        ch.old_blob, ch.new_blob,
                        (SELECT COUNT(*) FROM commit_parents p WHERE p.commit_oid = c.oid)
                 FROM commits AS c
                 JOIN changes AS ch ON ch.commit_oid = c.oid
                 WHERE c.oid IN (
                     SELECT anchored.commit_oid FROM changes AS anchored
                     WHERE anchored.old_path = ?1 OR anchored.new_path = ?1
                 )
                 ORDER BY c.rowid DESC, ch.ordinal",
            )
            .map_err(|error| search_error("preparing anchored history", error))?;
        let rows = statement
            .query_map(params![path], |row| {
                let message: Vec<u8> = row.get(3)?;
                let (subject, body) = message_parts(&message);
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    subject,
                    body,
                    PathChange {
                        ordinal: row.get(4)?,
                        status: row.get(5)?,
                        old_path: row.get(6)?,
                        new_path: row.get(7)?,
                        old_blob: row.get(8)?,
                        new_blob: row.get(9)?,
                    },
                    row.get::<_, i64>(10)?,
                ))
            })
            .map_err(|error| search_error("reading anchored history", error))?;

        for row in rows {
            let (position, oid, commit_time, subject, body, change, parent_count) =
                row.map_err(|error| search_error("reading anchored history", error))?;
            if !reachable.contains(&oid) {
                continue;
            }
            let anchored = change.old_path.as_deref() == Some(path.as_slice())
                || change.new_path.as_deref() == Some(path.as_slice());
            if change.status.starts_with('R') || change.status.starts_with('C') {
                if change.new_path.as_deref() == Some(path.as_slice()) {
                    if let Some(old_path) = &change.old_path {
                        pending.push(old_path.clone());
                    }
                } else if change.old_path.as_deref() == Some(path.as_slice())
                    && let Some(new_path) = &change.new_path
                {
                    pending.push(new_path.clone());
                }
            }
            let entry = commits.entry(oid.clone()).or_insert_with(|| HistoryCommit {
                position,
                oid,
                commit_time,
                subject,
                body,
                paths: Vec::new(),
                changes: Vec::new(),
                anchored_ordinals: Vec::new(),
                parent_count: usize::try_from(parent_count).unwrap_or(usize::MAX),
            });
            if anchored && !entry.anchored_ordinals.contains(&change.ordinal) {
                entry.anchored_ordinals.push(change.ordinal);
            }
            if !entry
                .changes
                .iter()
                .any(|candidate| candidate.ordinal == change.ordinal)
            {
                for candidate_path in [&change.old_path, &change.new_path].into_iter().flatten() {
                    if !entry
                        .paths
                        .iter()
                        .any(|existing| existing == candidate_path)
                    {
                        entry.paths.push(candidate_path.clone());
                    }
                }
                entry.changes.push(change);
            }
        }
    }

    let mut result = commits.into_values().collect::<Vec<_>>();
    result.sort_by(|left, right| {
        right
            .position
            .cmp(&left.position)
            .then_with(|| left.oid.cmp(&right.oid))
    });
    for commit in &mut result {
        commit.changes.sort_by_key(|change| change.ordinal);
    }
    Ok(result)
}

pub(in crate::analysis) fn ancestors(
    connection: &Connection,
    oid: &str,
) -> Result<HashSet<String>, AppError> {
    let mut statement = connection
        .prepare("SELECT parent_oid FROM commit_parents WHERE commit_oid = ?1 ORDER BY position")
        .map_err(|error| search_error("preparing commit ancestry", error))?;
    let mut pending = vec![oid.to_owned()];
    let mut ancestors = HashSet::new();
    while let Some(current) = pending.pop() {
        if !ancestors.insert(current.clone()) {
            continue;
        }
        let parents = statement
            .query_map([current.as_str()], |row| row.get::<_, String>(0))
            .map_err(|error| search_error("reading commit ancestry", error))?;
        for parent in parents {
            pending.push(parent.map_err(|error| search_error("reading commit ancestry", error))?);
        }
    }
    Ok(ancestors)
}

pub(in crate::analysis) fn hunks(
    connection: &Connection,
    oid: &str,
) -> Result<Vec<HistoryHunk>, AppError> {
    let mut statement = connection
        .prepare(
            "SELECT change_ordinal, old_start, old_lines, new_start, new_lines, text
             FROM hunks WHERE commit_oid = ?1 ORDER BY change_ordinal, ordinal",
        )
        .map_err(|error| search_error("preparing anchored hunks", error))?;
    statement
        .query_map([oid], |row| {
            Ok(HistoryHunk {
                change_ordinal: row.get(0)?,
                old_start: row.get(1)?,
                old_lines: row.get(2)?,
                new_start: row.get(3)?,
                new_lines: row.get(4)?,
                text: row.get(5)?,
            })
        })
        .map_err(|error| search_error("reading anchored hunks", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| search_error("reading anchored hunks", error))
}

pub(in crate::analysis) fn has_missing_objects(
    connection: &Connection,
    commits: &[HistoryCommit],
) -> Result<bool, AppError> {
    let mut statement = connection
        .prepare("SELECT EXISTS(SELECT 1 FROM missing_objects WHERE oid = ?1)")
        .map_err(|error| search_error("preparing anchored object check", error))?;
    for commit in commits {
        for change in &commit.changes {
            if !commit.anchored_ordinals.contains(&change.ordinal) {
                continue;
            }
            for blob in [&change.old_blob, &change.new_blob].into_iter().flatten() {
                let missing: i64 = statement
                    .query_row([blob], |row| row.get(0))
                    .map_err(|error| search_error("checking anchored objects", error))?;
                if missing != 0 {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}
