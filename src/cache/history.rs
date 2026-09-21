use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, params};

use crate::app::AppError;

use super::{QuerySession, cache_error, message_parts, read_hunks};

pub(crate) struct HistoryCommit {
    pub(crate) position: i64,
    pub(crate) oid: String,
    pub(crate) commit_time: i64,
    pub(crate) subject: String,
    pub(crate) body: String,
    pub(crate) paths: Vec<Vec<u8>>,
    pub(crate) changes: Vec<PathChange>,
    pub(crate) anchored_ordinals: Vec<i64>,
    pub(crate) parent_count: usize,
}

pub(crate) struct PathChange {
    pub(crate) ordinal: i64,
    pub(crate) status: String,
    pub(crate) old_path: Option<Vec<u8>>,
    pub(crate) new_path: Option<Vec<u8>>,
    pub(crate) old_blob: Option<String>,
    pub(crate) new_blob: Option<String>,
}

pub(crate) struct HistoryHunk {
    pub(crate) change_ordinal: i64,
    pub(crate) old_start: i64,
    pub(crate) old_lines: i64,
    pub(crate) new_start: i64,
    pub(crate) new_lines: i64,
    pub(crate) text: Vec<u8>,
}

impl QuerySession {
    pub(crate) fn path_history(
        &self,
        path: &[u8],
        reachable: &HashSet<String>,
    ) -> Result<Vec<HistoryCommit>, AppError> {
        path_history(&self.connection, path, reachable)
    }

    pub(crate) fn ancestors(&self, oid: &str) -> Result<HashSet<String>, AppError> {
        ancestors(&self.connection, oid)
    }

    pub(crate) fn history_hunks(&self, oid: &str) -> Result<Vec<HistoryHunk>, AppError> {
        hunks(&self.connection, oid)
    }

    pub(crate) fn has_missing_objects(&self, commits: &[HistoryCommit]) -> Result<bool, AppError> {
        has_missing_objects(&self.connection, commits)
    }
}

fn search_error(operation: &str, error: impl std::fmt::Display) -> AppError {
    cache_error(operation, error)
}

fn path_history(
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
                "SELECT c.position, c.oid, c.commit_time, c.message, c.message_length,
                        ch.ordinal, ch.status, ch.old_path, ch.new_path,
                        ch.old_blob, ch.new_blob,
                        (SELECT COUNT(*) FROM commit_parents p WHERE p.commit_id = c.commit_id)
                 FROM commits AS c
                 JOIN changes AS ch ON ch.commit_id = c.commit_id
                 WHERE c.commit_id IN (
                     SELECT anchored.commit_id FROM changes AS anchored
                     WHERE anchored.old_path = ?1 OR anchored.new_path = ?1
                 )
                 ORDER BY c.position DESC, ch.ordinal",
            )
            .map_err(|error| search_error("preparing anchored history", error))?;
        let rows = statement
            .query_map(params![path], |row| {
                let message = super::decode_message_row(row, 3, 4)?;
                let (subject, body) = message_parts(&message);
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    subject,
                    body,
                    PathChange {
                        ordinal: row.get(5)?,
                        status: row.get(6)?,
                        old_path: row.get(7)?,
                        new_path: row.get(8)?,
                        old_blob: row.get(9)?,
                        new_blob: row.get(10)?,
                    },
                    row.get::<_, i64>(11)?,
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

fn ancestors(connection: &Connection, oid: &str) -> Result<HashSet<String>, AppError> {
    let mut statement = connection
        .prepare(
            "SELECT COALESCE(parent.oid, p.external_oid)
             FROM commit_parents AS p
             LEFT JOIN commits AS parent ON parent.commit_id = p.parent_id
             WHERE p.commit_id = (SELECT commit_id FROM commits WHERE oid = ?1)
             ORDER BY p.position",
        )
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

fn hunks(connection: &Connection, oid: &str) -> Result<Vec<HistoryHunk>, AppError> {
    read_hunks(connection, oid).map(|hunks| {
        hunks
            .into_iter()
            .map(|hunk| HistoryHunk {
                change_ordinal: hunk.change_ordinal,
                old_start: hunk.old_start,
                old_lines: hunk.old_lines,
                new_start: hunk.new_start,
                new_lines: hunk.new_lines,
                text: hunk.text,
            })
            .collect()
    })
}

fn has_missing_objects(
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
