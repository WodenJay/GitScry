//! Path touches and corrective follow-ups used to interpret abandonment.
//!
//! Bounded file-incarnation traversal belongs to cache::followups instead.

use rusqlite::{Connection, params_from_iter, types::Value};

use super::{SQL_PARAMETER_LIMIT, commits::StoredCommit, numbered_placeholders, search_error};
use crate::app::AppError;
use crate::cache::scope::{SEARCH_SCOPE_CTE, SearchFilter, scope_values};
use crate::cache::{QuerySession, decode_message_row};
use std::collections::HashSet;

impl QuerySession {
    pub(crate) fn follow_ups(
        &self,
        revert_oid: &str,
        path_ids: &[Vec<u8>],
        scope: Option<&SearchFilter>,
    ) -> Result<Vec<StoredCommit>, AppError> {
        follow_ups(&self.connection, revert_oid, path_ids, scope)
    }
}
/// Descendant commits touching the abandoned paths, in cache order.
fn follow_ups(
    connection: &Connection,
    revert_oid: &str,
    path_ids: &[Vec<u8>],
    scope: Option<&SearchFilter>,
) -> Result<Vec<StoredCommit>, AppError> {
    if path_ids.is_empty() {
        return Ok(Vec::new());
    }
    let scoped = scope.is_some();
    let path_chunk_limit = SQL_PARAMETER_LIMIT - if scoped { 5 } else { 1 };
    let mut candidates = Vec::<(i64, String, Vec<u8>)>::new();
    let mut seen = HashSet::new();
    for path_chunk in path_ids.chunks(path_chunk_limit) {
        let path_placeholders = numbered_placeholders(if scoped { 6 } else { 2 }, path_chunk.len());
        let (query, values) = if let Some(scope) = scope {
            (
                format!(
                    "{SEARCH_SCOPE_CTE}, descendants(commit_id) AS (
                         SELECT child.commit_id
                         FROM commit_parents AS child
                         WHERE child.parent_id = (SELECT commit_id FROM commits WHERE oid = ?5)
                         UNION
                         SELECT child.commit_id
                         FROM commit_parents AS child
                         JOIN descendants ON child.parent_id = descendants.commit_id
                     )
                     SELECT c.position, c.oid, c.message, c.message_length
                     FROM commits AS c
                     JOIN eligible ON eligible.commit_id = c.commit_id
                     JOIN descendants ON descendants.commit_id = c.commit_id
                     JOIN commit_paths AS cp ON cp.commit_id = c.commit_id
                     WHERE c.position > (SELECT position FROM commits WHERE oid = ?5)
                       AND c.oid <> ?5
                       AND cp.raw_path IN ({path_placeholders})
                     GROUP BY c.commit_id
                     ORDER BY c.position ASC"
                ),
                scope_values(scope)
                    .into_iter()
                    .chain(std::iter::once(Value::Text(revert_oid.to_owned())))
                    .chain(path_chunk.iter().cloned().map(Value::Blob))
                    .collect::<Vec<_>>(),
            )
        } else {
            (
                format!(
                    "WITH RECURSIVE descendants(commit_id) AS (
                         SELECT child.commit_id
                         FROM commit_parents AS child
                         WHERE child.parent_id = (SELECT commit_id FROM commits WHERE oid = ?1)
                         UNION
                         SELECT child.commit_id
                         FROM commit_parents AS child
                         JOIN descendants ON child.parent_id = descendants.commit_id
                     )
                     SELECT c.position, c.oid, c.message, c.message_length
                     FROM commits AS c
                     JOIN descendants ON descendants.commit_id = c.commit_id
                     JOIN commit_paths AS cp ON cp.commit_id = c.commit_id
                     WHERE c.position > (SELECT position FROM commits WHERE oid = ?1)
                       AND c.oid <> ?1
                       AND cp.raw_path IN ({path_placeholders})
                     GROUP BY c.commit_id
                     ORDER BY c.position ASC"
                ),
                std::iter::once(Value::Text(revert_oid.to_owned()))
                    .chain(path_chunk.iter().cloned().map(Value::Blob))
                    .collect::<Vec<_>>(),
            )
        };
        let mut statement = connection
            .prepare(&query)
            .map_err(|error| search_error("preparing corrective follow-up", error))?;
        let rows = statement
            .query_map(params_from_iter(values), |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    decode_message_row(row, 2, 3)?,
                ))
            })
            .map_err(|error| search_error("reading corrective follow-up", error))?;
        for row in rows {
            let (position, oid, message) =
                row.map_err(|error| search_error("reading corrective follow-up", error))?;
            if seen.insert(oid.clone()) {
                candidates.push((position, oid, message));
            }
        }
    }
    candidates.sort_unstable_by_key(|candidate| candidate.0);
    Ok(candidates
        .into_iter()
        .map(|(_, oid, message)| StoredCommit { oid, message })
        .collect())
}
