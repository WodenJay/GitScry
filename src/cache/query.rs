use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value};
use std::collections::{HashMap, HashSet};

use crate::app::AppError;

use super::{QuerySession, cache_error, decode_message_row, message_parts};

/// A lexical candidate: one cache row with the paths it changed.
pub(crate) struct SearchCandidate {
    pub(crate) commit_id: i64,
    pub(crate) position: i64,
    pub(crate) oid: String,
    pub(crate) commit_time: i64,
    pub(crate) subject: String,
    pub(crate) body: String,
    pub(crate) paths: Vec<Vec<u8>>,
    pub(crate) path_keys: Vec<String>,
    pub(crate) bm25: f64,
}

/// One cached commit's deduplicated changed paths, in cache order.
pub(crate) struct RelationSupport {
    pub(crate) oid: String,
    pub(crate) commit_time: i64,
}

pub(crate) struct RelationCandidate {
    pub(crate) path: Vec<u8>,
    pub(crate) key: String,
    pub(crate) total_touches: usize,
    pub(crate) supporting: Vec<RelationSupport>,
    pub(crate) seed_keys: HashSet<String>,
}

pub(crate) struct RelationHistory {
    pub(crate) candidates: HashMap<String, RelationCandidate>,
    pub(crate) seed_touch_commits: usize,
    pub(crate) eligible_commits: usize,
    pub(crate) mass_changes_filtered: bool,
}

pub(crate) struct StoredChange {
    pub(crate) status: String,
    pub(crate) old_path: Option<Vec<u8>>,
    pub(crate) new_path: Option<Vec<u8>>,
}
impl QuerySession {
    pub(crate) fn relation_history(
        &self,
        seed_keys: &[String],
        mass_change_path_limit: usize,
    ) -> Result<RelationHistory, AppError> {
        relation_history(&self.connection, seed_keys, mass_change_path_limit)
    }

    pub(crate) fn match_count(&self, match_query: &str) -> Result<usize, AppError> {
        match_count(&self.connection, match_query)
    }

    pub(crate) fn candidates(
        &self,
        match_query: &str,
        limit: i64,
    ) -> Result<Vec<SearchCandidate>, AppError> {
        candidates(&self.connection, match_query, limit)
    }

    pub(crate) fn projected_path_keys(&self, oid: &str) -> Result<Vec<String>, AppError> {
        projected_path_keys(&self.connection, oid)
    }

    pub(crate) fn changes(&self, oid: &str) -> Result<Vec<StoredChange>, AppError> {
        changes(&self.connection, oid)
    }

    pub(crate) fn commit_message(&self, oid: &str) -> Result<Option<Vec<u8>>, AppError> {
        commit_message(&self.connection, oid)
    }

    pub(crate) fn commits(&self) -> Result<Vec<StoredCommit>, AppError> {
        commits(&self.connection)
    }

    pub(crate) fn touched_between(
        &self,
        from_position: i64,
        to_position: i64,
        path_keys: &[String],
    ) -> Result<bool, AppError> {
        touch_between(&self.connection, from_position, to_position, path_keys)
    }

    pub(crate) fn follow_ups(
        &self,
        revert_oid: &str,
        path_keys: &[String],
    ) -> Result<Vec<StoredCommit>, AppError> {
        follow_ups(&self.connection, revert_oid, path_keys)
    }
}

fn search_error(operation: &str, error: impl std::fmt::Display) -> AppError {
    cache_error(operation, error)
}

fn relation_history(
    connection: &Connection,
    seed_keys: &[String],
    mass_change_path_limit: usize,
) -> Result<RelationHistory, AppError> {
    if seed_keys.is_empty() {
        return Ok(RelationHistory {
            candidates: HashMap::new(),
            seed_touch_commits: 0,
            eligible_commits: 0,
            mass_changes_filtered: false,
        });
    }
    let limit = i64::try_from(mass_change_path_limit).map_err(|_| {
        search_error(
            "reading relation history",
            "path limit exceeded platform limits",
        )
    })?;
    let eligible_commits = relation_count(connection, limit)?;
    let seed_matches = relation_seed_matches(connection, seed_keys, limit)?;
    let seed_touch_commits = seed_matches.keys().copied().collect::<HashSet<_>>().len();
    let mass_changes_filtered = relation_has_mass_change(connection, seed_keys, limit)?;
    let mut candidates = HashMap::new();
    let seed_commit_ids = seed_matches.keys().copied().collect::<Vec<_>>();
    for commit_ids in seed_commit_ids.chunks(SQL_PARAMETER_LIMIT) {
        let placeholders = numbered_placeholders(1, commit_ids.len());
        let query = format!(
            "SELECT cp.commit_id, cp.path_key, cp.path_basename, cp.raw_path,\n\
             c.oid, c.commit_time\n\
             FROM commit_paths AS cp\n\
             JOIN commits AS c ON c.commit_id = cp.commit_id\n\
             WHERE cp.commit_id IN ({placeholders})\n\
             ORDER BY c.position, cp.path_order"
        );
        let values = commit_ids
            .iter()
            .copied()
            .map(Value::Integer)
            .collect::<Vec<_>>();
        let mut statement = connection
            .prepare(&query)
            .map_err(|error| search_error("preparing relation support lookup", error))?;
        let rows = statement
            .query_map(params_from_iter(values), |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(|error| search_error("reading relation support lookup", error))?;
        for row in rows {
            let (commit_id, key, basename, raw_path, oid, commit_time) =
                row.map_err(|error| search_error("reading relation support lookup", error))?;
            let Some(touched_seeds) = seed_matches.get(&commit_id) else {
                continue;
            };
            if touched_seeds
                .iter()
                .any(|seed| seed_matches_path(seed, &key, &basename))
            {
                continue;
            }
            let candidate = candidates
                .entry(key.clone())
                .or_insert_with(|| RelationCandidate {
                    path: raw_path,
                    key,
                    total_touches: 0,
                    supporting: Vec::new(),
                    seed_keys: HashSet::new(),
                });
            candidate.seed_keys.extend(touched_seeds.iter().cloned());
            candidate
                .supporting
                .push(RelationSupport { oid, commit_time });
        }
    }
    for keys in candidates
        .keys()
        .cloned()
        .collect::<Vec<_>>()
        .chunks(SQL_PARAMETER_LIMIT)
    {
        let placeholders = numbered_placeholders(2, keys.len());
        let query = format!(
            "WITH eligible AS (\n\
             SELECT pc.commit_id\n\
             FROM commit_path_counts AS pc\n\
             JOIN commits AS c ON c.commit_id = pc.commit_id\n\
             WHERE pc.path_count > 1 AND pc.path_count <= ?1\n\
               AND NOT EXISTS (\n\
                   SELECT 1 FROM commit_parents AS parents\n\
                   WHERE parents.commit_id = pc.commit_id AND parents.position > 0\n\
               )\n\
             ), ranked AS (\n\
             SELECT cp.path_key, cp.raw_path,\n\
                    COUNT(*) OVER (PARTITION BY cp.path_key) AS total_touches,\n\
                    ROW_NUMBER() OVER (\n\
                        PARTITION BY cp.path_key\n\
                        ORDER BY c.position, cp.path_order\n\
                    ) AS path_rank\n\
             FROM commit_paths AS cp\n\
             JOIN eligible ON eligible.commit_id = cp.commit_id\n\
             JOIN commits AS c ON c.commit_id = cp.commit_id\n\
             WHERE cp.path_key IN ({placeholders})\n\
             )\n\
             SELECT path_key, raw_path, total_touches\n\
             FROM ranked\n\
             WHERE path_rank = 1"
        );
        let mut values = vec![Value::Integer(limit)];
        values.extend(keys.iter().cloned().map(Value::Text));
        let mut statement = connection
            .prepare(&query)
            .map_err(|error| search_error("preparing relation touch lookup", error))?;
        let rows = statement
            .query_map(params_from_iter(values), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(|error| search_error("reading relation touch lookup", error))?;
        for row in rows {
            let (key, path, total_touches) =
                row.map_err(|error| search_error("reading relation touch lookup", error))?;
            if let Some(candidate) = candidates.get_mut(&key) {
                candidate.path = path;
                candidate.total_touches = usize::try_from(total_touches).map_err(|_| {
                    search_error(
                        "reading relation touch lookup",
                        "touch count exceeded platform limits",
                    )
                })?;
            }
        }
    }
    Ok(RelationHistory {
        candidates,
        seed_touch_commits,
        eligible_commits: usize::try_from(eligible_commits).map_err(|_| {
            search_error(
                "counting relation commits",
                "commit count exceeded platform limits",
            )
        })?,
        mass_changes_filtered,
    })
}

const SQL_PARAMETER_LIMIT: usize = 900;

fn relation_count(connection: &Connection, limit: i64) -> Result<i64, AppError> {
    connection
        .query_row(
            "SELECT COUNT(*)\n             FROM commit_path_counts AS pc\n             JOIN commits AS c ON c.commit_id = pc.commit_id\n             WHERE pc.path_count > 1 AND pc.path_count <= ?1\n               AND NOT EXISTS (\n                   SELECT 1 FROM commit_parents AS parents\n                   WHERE parents.commit_id = pc.commit_id AND parents.position > 0\n               )",
            [limit],
            |row| row.get(0),
        )
        .map_err(|error| search_error("counting relation commits", error))
}

fn relation_seed_matches(
    connection: &Connection,
    seed_keys: &[String],
    limit: i64,
) -> Result<HashMap<i64, HashSet<String>>, AppError> {
    let values_clause = seed_values_clause(seed_keys, 2);
    let query = format!(
        "WITH seed_keys(seed_key, is_basename) AS (VALUES {values_clause}),\n\
         eligible AS (\n\
             SELECT pc.commit_id\n\
             FROM commit_path_counts AS pc\n\
             WHERE pc.path_count > 1 AND pc.path_count <= ?1\n\
               AND NOT EXISTS (\n\
                   SELECT 1 FROM commit_parents AS parents\n\
                   WHERE parents.commit_id = pc.commit_id AND parents.position > 0\n\
               )\n\
         )\n\
         SELECT DISTINCT cp.commit_id, seed_keys.seed_key\n\
         FROM commit_paths AS cp\n\
         JOIN eligible ON eligible.commit_id = cp.commit_id\n\
         JOIN seed_keys\n\
           ON (seed_keys.is_basename = 1 AND cp.path_basename = seed_keys.seed_key)\n\
           OR (seed_keys.is_basename = 0 AND cp.path_key = seed_keys.seed_key)\n\
         ORDER BY cp.commit_id, seed_keys.seed_key"
    );
    let values = relation_seed_values(seed_keys, limit);
    let mut statement = connection
        .prepare(&query)
        .map_err(|error| search_error("preparing relation seed lookup", error))?;
    let rows = statement
        .query_map(params_from_iter(values), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| search_error("reading relation seed lookup", error))?;
    let mut matches = HashMap::<i64, HashSet<String>>::new();
    for row in rows {
        let (commit_id, seed_key) =
            row.map_err(|error| search_error("reading relation seed lookup", error))?;
        matches.entry(commit_id).or_default().insert(seed_key);
    }
    Ok(matches)
}

fn relation_has_mass_change(
    connection: &Connection,
    seed_keys: &[String],
    limit: i64,
) -> Result<bool, AppError> {
    let values_clause = seed_values_clause(seed_keys, 2);
    let query = format!(
        "WITH seed_keys(seed_key, is_basename) AS (VALUES {values_clause})\n\
         SELECT EXISTS(\n\
             SELECT 1\n\
             FROM commit_path_counts AS pc\n\
             WHERE pc.path_count > ?1\n\
               AND NOT EXISTS (\n\
                   SELECT 1 FROM commit_parents AS parents\n\
                   WHERE parents.commit_id = pc.commit_id AND parents.position > 0\n\
               )\n\
               AND EXISTS (\n\
                   SELECT 1 FROM commit_paths AS cp\n\
                   JOIN seed_keys\n\
                     ON (seed_keys.is_basename = 1 AND cp.path_basename = seed_keys.seed_key)\n\
                     OR (seed_keys.is_basename = 0 AND cp.path_key = seed_keys.seed_key)\n\
                   WHERE cp.commit_id = pc.commit_id\n\
               )\n\
         )"
    );
    let found: i64 = connection
        .query_row(
            &query,
            params_from_iter(relation_seed_values(seed_keys, limit)),
            |row| row.get(0),
        )
        .map_err(|error| search_error("checking mass relation changes", error))?;
    Ok(found != 0)
}

fn seed_values_clause(seed_keys: &[String], first_parameter: usize) -> String {
    seed_keys
        .iter()
        .enumerate()
        .map(|(index, _)| {
            let key = first_parameter + index * 2;
            format!("(?{key}, ?{})", key + 1)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn relation_seed_values(seed_keys: &[String], limit: i64) -> Vec<Value> {
    let mut values = vec![Value::Integer(limit)];
    for seed in seed_keys {
        values.push(Value::Text(seed.clone()));
        values.push(Value::Integer((!seed.contains('/')) as i64));
    }
    values
}

fn numbered_placeholders(first: usize, count: usize) -> String {
    (first..first + count)
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn seed_matches_path(seed: &str, key: &str, basename: &str) -> bool {
    if seed.contains('/') {
        seed == key
    } else {
        seed == basename
    }
}

fn match_count(connection: &Connection, match_query: &str) -> Result<usize, AppError> {
    let count = connection
        .query_row(
            "SELECT COUNT(*) FROM search_fts WHERE search_fts MATCH ?1",
            [match_query],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| search_error("counting search matches", error))?;
    usize::try_from(count)
        .map_err(|_| search_error("counting search matches", "count exceeded platform limits"))
}

/// The strongest lexical candidates, each carrying the paths it changed.
fn candidates(
    connection: &Connection,
    match_query: &str,
    limit: i64,
) -> Result<Vec<SearchCandidate>, AppError> {
    let mut statement = connection
        .prepare(
            "SELECT c.commit_id, c.oid, c.commit_time, c.message, c.message_length,
                    bm25(search_fts, 10.0, 3.0, 2.0), c.position
             FROM search_fts
             JOIN commits AS c ON c.commit_id = search_fts.rowid
             WHERE search_fts MATCH ?1
             ORDER BY bm25(search_fts, 10.0, 3.0, 2.0), c.commit_time DESC, c.oid ASC
             LIMIT ?2",
        )
        .map_err(|error| search_error("preparing search", error))?;
    let rows = statement
        .query_map(params![match_query, limit], |row| {
            let message = decode_message_row(row, 3, 4)?;
            let (subject, body) = message_parts(&message);
            Ok(SearchCandidate {
                commit_id: row.get(0)?,
                oid: row.get(1)?,
                commit_time: row.get(2)?,
                subject,
                body,
                paths: Vec::new(),
                path_keys: Vec::new(),
                bm25: row.get(5)?,
                position: row.get(6)?,
            })
        })
        .map_err(|error| search_error("running search", error))?;
    let mut candidates = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| search_error("reading search results", error))?;
    load_candidate_paths(connection, &mut candidates)?;
    Ok(candidates)
}
fn load_candidate_paths(
    connection: &Connection,
    candidates: &mut [SearchCandidate],
) -> Result<(), AppError> {
    for candidates in candidates.chunks_mut(SQL_PARAMETER_LIMIT) {
        let commit_ids = candidates
            .iter()
            .map(|candidate| candidate.commit_id)
            .collect::<Vec<_>>();
        let placeholders = numbered_placeholders(1, commit_ids.len());
        let query = format!(
            "SELECT commit_id, old_path, new_path
             FROM changes
             WHERE commit_id IN ({placeholders})
             ORDER BY commit_id, ordinal"
        );
        let values = commit_ids.iter().copied().map(Value::Integer);
        let mut statement = connection
            .prepare(&query)
            .map_err(|error| search_error("preparing candidate paths", error))?;
        let rows = statement
            .query_map(params_from_iter(values), |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<Vec<u8>>>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                ))
            })
            .map_err(|error| search_error("reading candidate paths", error))?;
        let mut paths_by_commit = HashMap::<i64, Vec<Vec<u8>>>::new();
        for row in rows {
            let (commit_id, old_path, new_path) =
                row.map_err(|error| search_error("reading candidate paths", error))?;
            let paths = paths_by_commit.entry(commit_id).or_default();
            for path in [old_path, new_path].into_iter().flatten() {
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
        }
        let placeholders = numbered_placeholders(1, commit_ids.len());
        let query = format!(
            "SELECT commit_id, path_key
             FROM commit_paths
             WHERE commit_id IN ({placeholders})
             ORDER BY commit_id, path_order"
        );
        let values = commit_ids.iter().copied().map(Value::Integer);
        let mut statement = connection
            .prepare(&query)
            .map_err(|error| search_error("preparing candidate path projection", error))?;
        let rows = statement
            .query_map(params_from_iter(values), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| search_error("reading candidate path projection", error))?;
        let mut keys_by_commit = HashMap::<i64, Vec<String>>::new();
        for row in rows {
            let (commit_id, key) =
                row.map_err(|error| search_error("reading candidate path projection", error))?;
            keys_by_commit.entry(commit_id).or_default().push(key);
        }
        for candidate in candidates {
            candidate.paths = paths_by_commit
                .remove(&candidate.commit_id)
                .unwrap_or_default();
            candidate.path_keys = keys_by_commit
                .remove(&candidate.commit_id)
                .unwrap_or_default();
        }
    }
    Ok(())
}
fn projected_path_keys(connection: &Connection, oid: &str) -> Result<Vec<String>, AppError> {
    let mut statement = connection
        .prepare(
            "SELECT cp.path_key
             FROM commit_paths AS cp
             JOIN commits AS c ON c.commit_id = cp.commit_id
             WHERE c.oid = ?1
             ORDER BY cp.path_order",
        )
        .map_err(|error| search_error("preparing path projection", error))?;
    statement
        .query_map([oid], |row| row.get::<_, String>(0))
        .map_err(|error| search_error("reading path projection", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| search_error("reading path projection", error))
}

/// The raw path changes a commit made, in change order.
fn changes(connection: &Connection, oid: &str) -> Result<Vec<StoredChange>, AppError> {
    let mut statement = connection
        .prepare(
            "SELECT ch.status, ch.old_path, ch.new_path
             FROM changes AS ch
             JOIN commits AS c ON c.commit_id = ch.commit_id
             WHERE c.oid = ?1
             ORDER BY ch.ordinal",
        )
        .map_err(|error| search_error("preparing change shapes", error))?;
    statement
        .query_map([oid], |row| {
            Ok(StoredChange {
                status: row.get(0)?,
                old_path: row.get(1)?,
                new_path: row.get(2)?,
            })
        })
        .map_err(|error| search_error("reading change shapes", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| search_error("reading change shapes", error))
}

/// The decoded message of one cached commit.
fn commit_message(connection: &Connection, oid: &str) -> Result<Option<Vec<u8>>, AppError> {
    let mut statement = connection
        .prepare("SELECT message, message_length FROM commits WHERE oid = ?1")
        .map_err(|error| search_error("preparing commit lookup", error))?;
    let message = statement
        .query_row([oid], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?))
        })
        .optional()
        .map_err(|error| search_error("reading commit lookup", error))?;
    message
        .map(|(compressed, length)| super::decode_message(&compressed, length))
        .transpose()
}

/// A cached commit: its object ID, message, and cache position.
///
/// The position orders commits the way the cache generation was built, which separates
/// commits a repository recorded in the same second.
pub(crate) struct StoredCommit {
    pub(crate) position: i64,
    pub(crate) oid: String,
    pub(crate) message: Vec<u8>,
}

/// Every cached commit, oldest first.
fn commits(connection: &Connection) -> Result<Vec<StoredCommit>, AppError> {
    let mut statement = connection
        .prepare(
            "SELECT position, oid, message, message_length FROM commits ORDER BY commit_time, oid",
        )
        .map_err(|error| search_error("preparing history scan", error))?;
    statement
        .query_map([], |row| {
            Ok(StoredCommit {
                position: row.get(0)?,
                oid: row.get(1)?,
                message: decode_message_row(row, 2, 3)?,
            })
        })
        .map_err(|error| search_error("reading history scan", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| search_error("reading history scan", error))
}

/// Whether any commit strictly between two positions touched one of `path_keys`.
///
/// A revert only counts as undoing a candidate when that candidate was the last work on
/// the path; otherwise the revert belongs to some later, unrelated change.
fn touch_between(
    connection: &Connection,
    from_position: i64,
    to_position: i64,
    path_keys: &[String],
) -> Result<bool, AppError> {
    if path_keys.is_empty() || from_position >= to_position {
        return Ok(false);
    }
    for path_chunk in path_keys.chunks(SQL_PARAMETER_LIMIT) {
        let placeholders = std::iter::repeat_n("?", path_chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let query = format!(
            "SELECT EXISTS(SELECT 1 FROM commits AS c
             JOIN commit_paths AS cp ON cp.commit_id = c.commit_id
             WHERE c.position > ?1 AND c.position < ?2
               AND cp.path_key IN ({placeholders}))"
        );
        let values = [Value::Integer(from_position), Value::Integer(to_position)]
            .into_iter()
            .chain(path_chunk.iter().cloned().map(Value::Text));
        let touched: i64 = connection
            .query_row(&query, params_from_iter(values), |row| row.get(0))
            .map_err(|error| search_error("checking intervening history", error))?;
        if touched != 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Later commits touching the abandoned paths, in cache order.
fn follow_ups(
    connection: &Connection,
    revert_oid: &str,
    path_keys: &[String],
) -> Result<Vec<StoredCommit>, AppError> {
    if path_keys.is_empty() {
        return Ok(Vec::new());
    }
    let mut candidates = Vec::<(i64, String, Vec<u8>)>::new();
    let mut seen = HashSet::new();
    for path_chunk in path_keys.chunks(SQL_PARAMETER_LIMIT) {
        let placeholders = std::iter::repeat_n("?", path_chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let query = format!(
            "SELECT c.position, c.oid, c.message, c.message_length
             FROM commits AS c
             JOIN commit_paths AS cp ON cp.commit_id = c.commit_id
             WHERE c.position > (SELECT position FROM commits WHERE oid = ?1)
               AND c.oid <> ?1
               AND cp.path_key IN ({placeholders})
             GROUP BY c.commit_id
             ORDER BY c.position ASC"
        );
        let values = std::iter::once(Value::Text(revert_oid.to_owned()))
            .chain(path_chunk.iter().cloned().map(Value::Text));
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
        .map(|(position, oid, message)| StoredCommit {
            position,
            oid,
            message,
        })
        .collect())
}
