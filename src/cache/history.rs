use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, params};

use crate::app::AppError;

use super::scope::{SEARCH_SCOPE_CTE, SearchFilter};
use super::{HunkReader, QuerySession, cache_error, message_parts, read_hunks};

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
    pub(crate) shallow_boundary: bool,
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
    pub(crate) hunk_ordinal: i64,
    pub(crate) old_start: i64,
    pub(crate) old_lines: i64,
    pub(crate) new_start: i64,
    pub(crate) new_lines: i64,
    pub(crate) text: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct HunkId {
    pub(crate) change_ordinal: i64,
    pub(crate) hunk_ordinal: i64,
}

impl HistoryHunk {
    pub(crate) fn id(&self) -> HunkId {
        HunkId {
            change_ordinal: self.change_ordinal,
            hunk_ordinal: self.hunk_ordinal,
        }
    }
}

pub(crate) struct PatchHistory {
    pub(crate) missing_objects: bool,
    pub(crate) hunks: Vec<PatchHistoryHunk>,
    pub(crate) truncated: bool,
}

pub(crate) struct PatchHistoryHunk {
    pub(crate) change_ordinal: i64,
    pub(crate) old_path: Option<Vec<u8>>,
    pub(crate) new_path: Option<Vec<u8>>,
    pub(crate) old_start: i64,
    pub(crate) old_lines: i64,
    pub(crate) new_start: i64,
    pub(crate) new_lines: i64,
    pub(crate) hunk_ordinal: i64,
    pub(crate) text: Option<Vec<u8>>,
}

impl PatchHistoryHunk {
    pub(crate) fn id(&self) -> HunkId {
        HunkId {
            change_ordinal: self.change_ordinal,
            hunk_ordinal: self.hunk_ordinal,
        }
    }
}

pub(crate) struct CodeHunk {
    pub(crate) oid: String,
    pub(crate) commit_time: i64,
    pub(crate) change_ordinal: i64,
    pub(crate) old_path: Option<Vec<u8>>,
    pub(crate) new_path: Option<Vec<u8>>,
    pub(crate) hunk_ordinal: i64,
    pub(crate) old_start: i64,
    pub(crate) new_start: i64,
    pub(crate) text: Vec<u8>,
}

impl QuerySession {
    pub(crate) fn path_history(
        &self,
        path: &[u8],
        reachable: &HashSet<String>,
    ) -> Result<Vec<HistoryCommit>, AppError> {
        path_history(&self.connection, path, reachable, true)
    }

    pub(crate) fn timeline_history(
        &self,
        path: &[u8],
        reachable: &HashSet<String>,
    ) -> Result<Vec<HistoryCommit>, AppError> {
        path_history(&self.connection, path, reachable, false)
    }

    pub(crate) fn scoped_revisions(
        &self,
        scope: &SearchFilter,
        target_oid: &str,
    ) -> Result<HashSet<String>, AppError> {
        let mut statement = self
            .connection
            .prepare(
                r#"WITH RECURSIVE scope_reachable(commit_id) AS (
                    SELECT commit_id FROM commits WHERE oid = ?1
                    UNION
                    SELECT parent.parent_id
                    FROM commit_parents AS parent
                    JOIN scope_reachable ON scope_reachable.commit_id = parent.commit_id
                    WHERE parent.parent_id IS NOT NULL
                ), target_reachable(commit_id) AS (
                    SELECT commit_id FROM commits WHERE oid = ?2
                    UNION
                    SELECT parent.parent_id
                    FROM commit_parents AS parent
                    JOIN target_reachable ON target_reachable.commit_id = parent.commit_id
                    WHERE parent.parent_id IS NOT NULL
                ), excluded(commit_id) AS (
                    SELECT commit_id FROM commits WHERE oid = ?3
                    UNION
                    SELECT parent.parent_id
                    FROM commit_parents AS parent
                    JOIN excluded ON excluded.commit_id = parent.commit_id
                    WHERE parent.parent_id IS NOT NULL
                )
                SELECT commits.oid
                FROM commits
                WHERE commits.commit_id IN (SELECT commit_id FROM scope_reachable)
                  AND commits.commit_id IN (SELECT commit_id FROM target_reachable)
                  AND (?3 IS NULL OR commits.commit_id NOT IN (SELECT commit_id FROM excluded))
                  AND (?4 IS NULL OR commits.commit_time >= ?4)
                  AND (?5 IS NULL OR commits.commit_time <= ?5)
                ORDER BY commits.position DESC"#,
            )
            .map_err(|error| search_error("preparing scoped history revisions", error))?;
        let rows = statement
            .query_map(
                params![
                    scope.to_oid.as_str(),
                    target_oid,
                    scope.from_oid.as_deref(),
                    scope.since,
                    scope.until,
                ],
                |row| row.get::<_, String>(0),
            )
            .map_err(|error| search_error("querying scoped history revisions", error))?;
        rows.collect::<Result<HashSet<_>, _>>()
            .map_err(|error| search_error("reading scoped history revisions", error))
    }

    pub(crate) fn ancestors(&self, oid: &str) -> Result<HashSet<String>, AppError> {
        ancestors(&self.connection, oid)
    }

    pub(crate) fn first_parent_ancestors(&self, oid: &str) -> Result<HashSet<String>, AppError> {
        first_parent_ancestors(&self.connection, oid)
    }

    pub(crate) fn history_hunks(&self, oid: &str) -> Result<Vec<HistoryHunk>, AppError> {
        hunks(&self.connection, oid)
    }

    pub(crate) fn patch_history(
        &self,
        oid: &str,
        max_hunks: usize,
        max_hunk_bytes: usize,
    ) -> Result<PatchHistory, AppError> {
        patch_history(&self.connection, oid, None, max_hunks, max_hunk_bytes)
    }

    pub(crate) fn patch_history_for_change(
        &self,
        oid: &str,
        change_ordinal: i64,
        max_hunks: usize,
        max_hunk_bytes: usize,
    ) -> Result<PatchHistory, AppError> {
        patch_history(
            &self.connection,
            oid,
            Some(change_ordinal),
            max_hunks,
            max_hunk_bytes,
        )
    }

    pub(crate) fn scan_code_hunks(
        &self,
        visit: impl FnMut(CodeHunk) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        scan_code_hunks(&self.connection, visit)
    }

    pub(crate) fn scan_code_hunks_scoped(
        &self,
        scope: &SearchFilter,
        visit: impl FnMut(CodeHunk) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        scan_code_hunks_scoped(&self.connection, scope, visit)
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
    follow_copies: bool,
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
                        (SELECT COUNT(*) FROM commit_parents p WHERE p.commit_id = c.commit_id),
                        EXISTS(SELECT 1 FROM shallow_boundaries b WHERE b.oid = c.oid)
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
                    row.get::<_, bool>(12)?,
                ))
            })
            .map_err(|error| search_error("reading anchored history", error))?;

        for row in rows {
            let (position, oid, commit_time, subject, body, change, parent_count, shallow_boundary) =
                row.map_err(|error| search_error("reading anchored history", error))?;
            if !reachable.contains(&oid) {
                continue;
            }
            let is_rename = change.status.starts_with('R');
            let is_copy = change.status.starts_with('C');
            let is_source_copy = !follow_copies
                && is_copy
                && change.old_path.as_deref() == Some(path.as_slice())
                && change.new_path.as_deref() != Some(path.as_slice());
            if is_source_copy {
                continue;
            }
            let anchored = change.old_path.as_deref() == Some(path.as_slice())
                || change.new_path.as_deref() == Some(path.as_slice());
            if is_rename || (follow_copies && is_copy) {
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
                shallow_boundary,
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

fn first_parent_ancestors(connection: &Connection, oid: &str) -> Result<HashSet<String>, AppError> {
    let mut statement = connection
        .prepare(
            "WITH RECURSIVE first_parent_chain(commit_id) AS (
                SELECT commit_id FROM commits WHERE oid = ?1
                UNION ALL
                SELECT parent.parent_id
                FROM commit_parents AS parent
                JOIN first_parent_chain AS child ON child.commit_id = parent.commit_id
                WHERE parent.position = 0 AND parent.parent_id IS NOT NULL
            )
            SELECT commits.oid
            FROM first_parent_chain
            JOIN commits ON commits.commit_id = first_parent_chain.commit_id",
        )
        .map_err(|error| search_error("preparing first-parent history", error))?;
    let rows = statement
        .query_map([oid], |row| row.get::<_, String>(0))
        .map_err(|error| search_error("querying first-parent history", error))?;
    rows.collect::<Result<HashSet<_>, _>>()
        .map_err(|error| search_error("reading first-parent history", error))
}

fn hunks(connection: &Connection, oid: &str) -> Result<Vec<HistoryHunk>, AppError> {
    read_hunks(connection, oid).map(|hunks| {
        hunks
            .into_iter()
            .map(|hunk| HistoryHunk {
                change_ordinal: hunk.change_ordinal,
                hunk_ordinal: hunk.hunk_ordinal,
                old_start: hunk.old_start,
                old_lines: hunk.old_lines,
                new_start: hunk.new_start,
                new_lines: hunk.new_lines,
                text: hunk.text,
            })
            .collect()
    })
}

fn patch_history(
    connection: &Connection,
    oid: &str,
    change_ordinal: Option<i64>,
    max_hunks: usize,
    max_hunk_bytes: usize,
) -> Result<PatchHistory, AppError> {
    let missing_objects: bool = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM changes AS ch
                 JOIN commits AS c ON c.commit_id = ch.commit_id
                 WHERE c.oid = ?1 AND (?2 IS NULL OR ch.ordinal = ?2) AND (
                     EXISTS (SELECT 1 FROM missing_objects AS m WHERE m.oid = ch.old_blob)
                     OR EXISTS (SELECT 1 FROM missing_objects AS m WHERE m.oid = ch.new_blob)
                 )
             )",
            params![oid, change_ordinal],
            |row| row.get(0),
        )
        .map_err(|error| search_error("checking cached patch completeness", error))?;

    let limit = i64::try_from(max_hunks.saturating_add(1)).unwrap_or(i64::MAX);
    let mut statement = connection
        .prepare(
            "SELECT ch.ordinal, ch.old_path, ch.new_path, h.ordinal,
                    h.old_start, h.old_lines, h.new_start, h.new_lines, h.payload_id
             FROM commits AS c
             JOIN changes AS ch ON ch.commit_id = c.commit_id
             JOIN hunks AS h ON h.change_id = ch.change_id
             WHERE c.oid = ?1 AND (?2 IS NULL OR ch.ordinal = ?2)
             ORDER BY ch.ordinal, h.ordinal
             LIMIT ?3",
        )
        .map_err(|error| search_error("preparing cached patch hunks", error))?;
    let rows = statement
        .query_map(params![oid, change_ordinal, limit], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<Vec<u8>>>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
            ))
        })
        .map_err(|error| search_error("reading cached patch hunks", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| search_error("reading cached patch hunks", error))?;
    drop(statement);

    let mut reader = HunkReader::new(connection)?;
    let mut hunks = Vec::new();
    let mut truncated = rows.len() > max_hunks;
    for (
        change_ordinal,
        old_path,
        new_path,
        hunk_ordinal,
        old_start,
        old_lines,
        new_start,
        new_lines,
        payload_id,
    ) in rows.into_iter().take(max_hunks)
    {
        let material = format!("{oid}/change {change_ordinal}/hunk {hunk_ordinal}");
        let text = reader.decode_payload_limited(payload_id, &material, max_hunk_bytes)?;
        truncated |= text.is_none();
        reader.clear_decoded_blocks();
        hunks.push(PatchHistoryHunk {
            old_path,
            change_ordinal,
            hunk_ordinal,
            new_path,
            old_start,
            old_lines,
            new_start,
            new_lines,
            text,
        });
    }

    Ok(PatchHistory {
        missing_objects,
        hunks,
        truncated,
    })
}

fn scan_code_hunks(
    connection: &Connection,
    visit: impl FnMut(CodeHunk) -> Result<(), AppError>,
) -> Result<(), AppError> {
    scan_code_hunks_inner(connection, None, visit)
}

fn scan_code_hunks_scoped(
    connection: &Connection,
    scope: &SearchFilter,
    visit: impl FnMut(CodeHunk) -> Result<(), AppError>,
) -> Result<(), AppError> {
    scan_code_hunks_inner(connection, Some(scope), visit)
}

type StoredCodeHunk = (
    String,
    i64,
    i64,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    i64,
    i64,
    i64,
    i64,
    i64,
);

fn code_hunk_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredCodeHunk> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
    ))
}

fn scan_code_hunks_inner(
    connection: &Connection,
    scope: Option<&SearchFilter>,
    mut visit: impl FnMut(CodeHunk) -> Result<(), AppError>,
) -> Result<(), AppError> {
    let mut reader = HunkReader::new(connection)?;
    let query = match scope {
        Some(_) => format!(
            "{SEARCH_SCOPE_CTE}
             SELECT c.oid, c.commit_time, ch.ordinal, ch.old_path, ch.new_path,
                    h.ordinal, h.old_start, h.new_start, h.payload_id, p.token_block_id
             FROM hunks AS h
             JOIN changes AS ch ON ch.change_id = h.change_id
             JOIN commits AS c ON c.commit_id = ch.commit_id
             JOIN hunk_payloads AS p ON p.payload_id = h.payload_id
             WHERE c.commit_id IN (SELECT commit_id FROM eligible)
             ORDER BY p.token_block_id, p.token_offset"
        ),
        None => String::from(
            "SELECT c.oid, c.commit_time, ch.ordinal, ch.old_path, ch.new_path,
                    h.ordinal, h.old_start, h.new_start, h.payload_id, p.token_block_id
             FROM hunks AS h
             JOIN changes AS ch ON ch.change_id = h.change_id
             JOIN commits AS c ON c.commit_id = ch.commit_id
             JOIN hunk_payloads AS p ON p.payload_id = h.payload_id
             ORDER BY p.token_block_id, p.token_offset",
        ),
    };
    let mut statement = connection
        .prepare(&query)
        .map_err(|error| search_error("preparing code-search hunks", error))?;
    let rows = match scope {
        Some(scope) => statement.query_map(
            params![
                scope.to_oid.as_str(),
                scope.from_oid.as_deref(),
                scope.since,
                scope.until,
            ],
            code_hunk_row,
        ),
        None => statement.query_map([], code_hunk_row),
    }
    .map_err(|error| search_error("reading code-search hunks", error))?;
    let mut active_token_block = None;
    for row in rows {
        let (
            oid,
            commit_time,
            change_ordinal,
            old_path,
            new_path,
            hunk_ordinal,
            old_start,
            new_start,
            payload_id,
            token_block_id,
        ) = row.map_err(|error| search_error("reading code-search hunks", error))?;
        if active_token_block != Some(token_block_id) {
            reader.clear_decoded_blocks();
            active_token_block = Some(token_block_id);
        }
        let material = format!("{oid}/change {change_ordinal}/hunk {hunk_ordinal}");
        let text = reader.decode_payload(payload_id, &material)?;
        visit(CodeHunk {
            oid,
            commit_time,
            change_ordinal,
            old_path,
            new_path,
            hunk_ordinal,
            old_start,
            new_start,
            text,
        })?;
    }
    Ok(())
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
