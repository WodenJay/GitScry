use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value};
use std::{
    cmp::Ordering,
    collections::{BTreeSet, HashMap, HashSet},
};

use crate::app::AppError;

use super::{QuerySession, cache_error, decode_message_row, message_parts};

#[derive(Clone, Debug)]
pub(crate) struct SearchFilter {
    pub(crate) from_oid: Option<String>,
    pub(crate) to_oid: String,
    pub(crate) since: Option<i64>,
    pub(crate) until: Option<i64>,
}

pub(crate) const SEARCH_SCOPE_CTE: &str = r#"
WITH RECURSIVE reachable(commit_id) AS (
    SELECT commit_id FROM commits WHERE oid = ?1
    UNION
    SELECT parent.parent_id
    FROM commit_parents AS parent
    JOIN reachable ON reachable.commit_id = parent.commit_id
    WHERE parent.parent_id IS NOT NULL
), excluded(commit_id) AS (
    SELECT commit_id FROM commits WHERE oid = ?2
    UNION
    SELECT parent.parent_id
    FROM commit_parents AS parent
    JOIN excluded ON excluded.commit_id = parent.commit_id
    WHERE parent.parent_id IS NOT NULL
), eligible(commit_id) AS (
    SELECT commits.commit_id
    FROM commits
    WHERE commits.commit_id IN (SELECT commit_id FROM reachable)
      AND (?2 IS NULL OR commits.commit_id NOT IN (SELECT commit_id FROM excluded))
      AND (?3 IS NULL OR commits.commit_time >= ?3)
      AND (?4 IS NULL OR commits.commit_time <= ?4)
 )
"#;

fn scope_values(scope: &SearchFilter) -> [Value; 4] {
    [
        Value::Text(scope.to_oid.clone()),
        scope.from_oid.clone().map_or(Value::Null, Value::Text),
        scope.since.map_or(Value::Null, Value::Integer),
        scope.until.map_or(Value::Null, Value::Integer),
    ]
}
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

#[derive(Clone, Debug)]
pub(crate) struct SemanticCandidate {
    pub(crate) commit_id: i64,
    pub(crate) oid: String,
    pub(crate) commit_time: i64,
    pub(crate) cosine: f64,
}

impl PartialEq for SemanticCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for SemanticCandidate {}

impl PartialOrd for SemanticCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SemanticCandidate {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .cosine
            .total_cmp(&self.cosine)
            .then_with(|| self.oid.cmp(&other.oid))
            .then_with(|| self.commit_id.cmp(&other.commit_id))
    }
}

pub(crate) struct SearchMaterial {
    pub(crate) commit_id: i64,
    pub(crate) oid: String,
    pub(crate) commit_time: i64,
    pub(crate) subject: String,
    pub(crate) paths: Vec<Vec<u8>>,
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
        scope: Option<&SearchFilter>,
    ) -> Result<RelationHistory, AppError> {
        relation_history(&self.connection, seed_keys, mass_change_path_limit, scope)
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

    pub(crate) fn match_count_scoped(
        &self,
        match_query: &str,
        scope: &SearchFilter,
    ) -> Result<usize, AppError> {
        match_count_scoped(&self.connection, match_query, scope)
    }

    pub(crate) fn candidates_scoped(
        &self,
        match_query: &str,
        scope: &SearchFilter,
    ) -> Result<Vec<SearchCandidate>, AppError> {
        candidates_scoped(&self.connection, match_query, scope)
    }

    pub(crate) fn require_semantic_ready(&self) -> Result<(), AppError> {
        super::semantic::require_ready_for_query(&self.connection)
    }

    pub(crate) fn semantic_top_k(
        &self,
        query_vectors: &[Vec<f32>],
        limit: usize,
        scope: Option<&SearchFilter>,
    ) -> Result<Vec<SemanticCandidate>, AppError> {
        semantic_top_k(&self.connection, query_vectors, limit, scope)
    }

    pub(crate) fn semantic_materials(
        &self,
        commit_ids: &[i64],
    ) -> Result<Vec<SearchMaterial>, AppError> {
        semantic_materials(&self.connection, commit_ids)
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

    pub(crate) fn commits_scoped(
        &self,
        scope: &SearchFilter,
    ) -> Result<Vec<StoredCommit>, AppError> {
        commits_scoped(&self.connection, scope)
    }

    pub(crate) fn commit_oids(&self) -> Result<HashSet<String>, AppError> {
        commit_oids(&self.connection)
    }

    pub(crate) fn touched_between(
        &self,
        from_position: i64,
        to_position: i64,
        path_keys: &[String],
        scope: Option<&SearchFilter>,
    ) -> Result<bool, AppError> {
        touch_between(
            &self.connection,
            from_position,
            to_position,
            path_keys,
            scope,
        )
    }

    pub(crate) fn follow_ups(
        &self,
        revert_oid: &str,
        path_keys: &[String],
        scope: Option<&SearchFilter>,
    ) -> Result<Vec<StoredCommit>, AppError> {
        follow_ups(&self.connection, revert_oid, path_keys, scope)
    }
}

fn search_error(operation: &str, error: impl std::fmt::Display) -> AppError {
    cache_error(operation, error)
}

fn relation_history(
    connection: &Connection,
    seed_keys: &[String],
    mass_change_path_limit: usize,
    scope: Option<&SearchFilter>,
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
    let eligible_commits = relation_count(connection, limit, scope)?;
    let seed_matches = relation_seed_matches(connection, seed_keys, limit, scope)?;
    let seed_touch_commits = seed_matches.keys().copied().collect::<HashSet<_>>().len();
    let mass_changes_filtered = relation_has_mass_change(connection, seed_keys, limit, scope)?;
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
        let query_bindings = RelationQueryBindings::new(scope, &[], limit);
        let scope_cte = &query_bindings.cte_prefix;
        let limit_parameter = query_bindings.limit_parameter;
        let placeholders = numbered_placeholders(query_bindings.first_seed_parameter, keys.len());
        let scope_predicate = query_bindings.scope_predicate;
        let query = format!(
            "{scope_cte}\n\
             relation_eligible AS (\n\
             SELECT pc.commit_id\n\
             FROM commit_path_counts AS pc\n\
             JOIN commits AS c ON c.commit_id = pc.commit_id\n\
             WHERE pc.path_count > 1 AND pc.path_count <= ?{limit_parameter}\n\
               {scope_predicate}\n\
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
             JOIN relation_eligible ON relation_eligible.commit_id = cp.commit_id\n\
             JOIN commits AS c ON c.commit_id = cp.commit_id\n\
             WHERE cp.path_key IN ({placeholders})\n\
             )\n\
             SELECT path_key, raw_path, total_touches\n\
             FROM ranked\n\
             WHERE path_rank = 1"
        );
        let mut values = query_bindings.values;
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
fn relation_scope_parameter_count(scope: Option<&SearchFilter>) -> usize {
    if scope.is_some() { 4 } else { 0 }
}

struct RelationQueryBindings {
    cte_prefix: String,
    limit_parameter: usize,
    first_seed_parameter: usize,
    scope_predicate: &'static str,
    values: Vec<Value>,
}

impl RelationQueryBindings {
    fn new(scope: Option<&SearchFilter>, seed_keys: &[String], limit: i64) -> Self {
        let scope_parameter_count = relation_scope_parameter_count(scope);
        let limit_parameter = scope_parameter_count + 1;
        let first_seed_parameter = limit_parameter + 1;
        let cte_prefix = if scope.is_some() {
            format!("{SEARCH_SCOPE_CTE},")
        } else {
            "WITH".to_owned()
        };
        let scope_predicate = if scope.is_some() {
            "AND pc.commit_id IN (SELECT commit_id FROM eligible)"
        } else {
            ""
        };

        let mut values = Vec::new();
        if let Some(scope) = scope {
            values.push(Value::Text(scope.to_oid.clone()));
            values.push(
                scope
                    .from_oid
                    .clone()
                    .map(Value::Text)
                    .unwrap_or(Value::Null),
            );
            values.push(scope.since.map(Value::Integer).unwrap_or(Value::Null));
            values.push(scope.until.map(Value::Integer).unwrap_or(Value::Null));
        }
        values.push(Value::Integer(limit));
        for seed in seed_keys {
            values.push(Value::Text(seed.clone()));
            values.push(Value::Integer((!seed.contains('/')) as i64));
        }

        Self {
            cte_prefix,
            limit_parameter,
            first_seed_parameter,
            scope_predicate,
            values,
        }
    }
}

fn relation_count(
    connection: &Connection,
    limit: i64,
    scope: Option<&SearchFilter>,
) -> Result<i64, AppError> {
    let query_bindings = RelationQueryBindings::new(scope, &[], limit);
    let scope_cte = &query_bindings.cte_prefix;
    let limit_parameter = query_bindings.limit_parameter;
    let scope_predicate = query_bindings.scope_predicate;
    let query = format!(
        "{scope_cte}\n\
         relation_eligible AS (\n\
         SELECT pc.commit_id\n\
         FROM commit_path_counts AS pc\n\
         JOIN commits AS c ON c.commit_id = pc.commit_id\n\
         WHERE pc.path_count > 1 AND pc.path_count <= ?{limit_parameter}\n\
           {scope_predicate}\n\
           AND NOT EXISTS (\n\
               SELECT 1 FROM commit_parents AS parents\n\
               WHERE parents.commit_id = pc.commit_id AND parents.position > 0\n\
           )\n\
         )\n\
         SELECT COUNT(*) FROM relation_eligible"
    );
    let values = query_bindings.values;
    connection
        .query_row(&query, params_from_iter(values), |row| row.get(0))
        .map_err(|error| search_error("counting relation commits", error))
}

fn relation_seed_matches(
    connection: &Connection,
    seed_keys: &[String],
    limit: i64,
    scope: Option<&SearchFilter>,
) -> Result<HashMap<i64, HashSet<String>>, AppError> {
    let query_bindings = RelationQueryBindings::new(scope, seed_keys, limit);
    let scope_cte = &query_bindings.cte_prefix;
    let limit_parameter = query_bindings.limit_parameter;
    let values_clause = seed_values_clause(seed_keys, query_bindings.first_seed_parameter);
    let scope_predicate = query_bindings.scope_predicate;
    let query = format!(
        "{scope_cte}\n\
         relation_eligible AS (\n\
             SELECT pc.commit_id\n\
             FROM commit_path_counts AS pc\n\
             WHERE pc.path_count > 1 AND pc.path_count <= ?{limit_parameter}\n\
               {scope_predicate}\n\
               AND NOT EXISTS (\n\
                   SELECT 1 FROM commit_parents AS parents\n\
                   WHERE parents.commit_id = pc.commit_id AND parents.position > 0\n\
               )\n\
         ), seed_keys(seed_key, is_basename) AS (VALUES {values_clause})\n\
         SELECT DISTINCT cp.commit_id, seed_keys.seed_key\n\
         FROM commit_paths AS cp\n\
         JOIN relation_eligible ON relation_eligible.commit_id = cp.commit_id\n\
         JOIN seed_keys\n\
           ON (seed_keys.is_basename = 1 AND cp.path_basename = seed_keys.seed_key)\n\
           OR (seed_keys.is_basename = 0 AND cp.path_key = seed_keys.seed_key)\n\
         ORDER BY cp.commit_id, seed_keys.seed_key"
    );
    let values = query_bindings.values;
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
    scope: Option<&SearchFilter>,
) -> Result<bool, AppError> {
    let query_bindings = RelationQueryBindings::new(scope, seed_keys, limit);
    let scope_cte = &query_bindings.cte_prefix;
    let limit_parameter = query_bindings.limit_parameter;
    let values_clause = seed_values_clause(seed_keys, query_bindings.first_seed_parameter);
    let scope_predicate = query_bindings.scope_predicate;
    let query = format!(
        "{scope_cte}\n\
         seed_keys(seed_key, is_basename) AS (VALUES {values_clause})\n\
         SELECT EXISTS(\n\
             SELECT 1\n\
             FROM commit_path_counts AS pc\n\
             WHERE pc.path_count > ?{limit_parameter}\n\
               {scope_predicate}\n\
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
        .query_row(&query, params_from_iter(query_bindings.values), |row| {
            row.get(0)
        })
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

fn match_count_scoped(
    connection: &Connection,
    match_query: &str,
    scope: &SearchFilter,
) -> Result<usize, AppError> {
    let query = format!(
        "{SEARCH_SCOPE_CTE}
         SELECT COUNT(*)
         FROM search_fts
         JOIN commits AS c ON c.commit_id = search_fts.rowid
         WHERE search_fts MATCH ?5
           AND c.commit_id IN (SELECT commit_id FROM eligible)"
    );
    let count = connection
        .query_row(
            &query,
            params![
                scope.to_oid.as_str(),
                scope.from_oid.as_deref(),
                scope.since,
                scope.until,
                match_query,
            ],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| search_error("counting scoped search matches", error))?;
    usize::try_from(count).map_err(|_| {
        search_error(
            "counting scoped search matches",
            "count exceeded platform limits",
        )
    })
}

fn candidates_scoped(
    connection: &Connection,
    match_query: &str,
    scope: &SearchFilter,
) -> Result<Vec<SearchCandidate>, AppError> {
    // FTS5 BM25 uses corpus-wide statistics, including out-of-scope commits. Search reranks
    // every scoped match with candidate-local signals before applying the result limit.
    let query = format!(
        "{SEARCH_SCOPE_CTE}
         SELECT c.commit_id, c.oid, c.commit_time, c.message, c.message_length,
                0.0, c.position
         FROM search_fts
         JOIN commits AS c ON c.commit_id = search_fts.rowid
         WHERE search_fts MATCH ?5
           AND c.commit_id IN (SELECT commit_id FROM eligible)
         ORDER BY c.commit_time DESC, c.oid ASC"
    );
    let mut statement = connection
        .prepare(&query)
        .map_err(|error| search_error("preparing scoped search", error))?;
    let rows = statement
        .query_map(
            params![
                scope.to_oid.as_str(),
                scope.from_oid.as_deref(),
                scope.since,
                scope.until,
                match_query,
            ],
            |row| {
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
            },
        )
        .map_err(|error| search_error("running scoped search", error))?;
    let mut candidates = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| search_error("reading scoped search results", error))?;
    load_candidate_paths(connection, &mut candidates)?;
    Ok(candidates)
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

const VECTOR_NORMALIZATION_TOLERANCE: f64 = 1e-3;

fn semantic_top_k(
    connection: &Connection,
    query_vectors: &[Vec<f32>],
    limit: usize,
    scope: Option<&SearchFilter>,
) -> Result<Vec<SemanticCandidate>, AppError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    validate_query_vectors(query_vectors)?;
    let encoder_fingerprint = crate::semantic::encoder_fingerprint();
    let sql = match scope {
        Some(_) => format!(
            "{SEARCH_SCOPE_CTE}
             SELECT c.commit_id, c.oid, c.commit_time, v.embedding,
                    v.source_fingerprint, v.input_fingerprint, v.encoder_fingerprint
             FROM eligible
             JOIN commits AS c ON c.commit_id = eligible.commit_id
             JOIN semantic_vectors AS v ON v.commit_id = c.commit_id
"
        ),
        None => "SELECT c.commit_id, c.oid, c.commit_time, v.embedding,
                       v.source_fingerprint, v.input_fingerprint, v.encoder_fingerprint
                FROM semantic_vectors AS v
                JOIN commits AS c ON c.commit_id = v.commit_id
"
        .to_owned(),
    };
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| search_error("preparing semantic retrieval", error))?;
    let mut rows = match scope {
        Some(scope) => statement
            .query(params_from_iter(scope_values(scope)))
            .map_err(|error| search_error("running scoped semantic retrieval", error))?,
        None => statement
            .query([])
            .map_err(|error| search_error("running semantic retrieval", error))?,
    };
    let mut top = BTreeSet::new();
    while let Some(row) = rows
        .next()
        .map_err(|error| search_error("reading semantic retrieval", error))?
    {
        let commit_id = row
            .get(0)
            .map_err(|error| search_error("reading semantic commit identity", error))?;
        let oid: String = row
            .get(1)
            .map_err(|error| search_error("reading semantic commit identity", error))?;
        let commit_time = row
            .get(2)
            .map_err(|error| search_error("reading semantic commit identity", error))?;
        let embedding: Vec<u8> = row
            .get(3)
            .map_err(|error| search_error("reading semantic vector", error))?;
        let source_fingerprint: String = row
            .get(4)
            .map_err(|error| search_error("reading semantic vector identity", error))?;
        let input_fingerprint: String = row
            .get(5)
            .map_err(|error| search_error("reading semantic vector identity", error))?;
        let stored_encoder_fingerprint: String = row
            .get(6)
            .map_err(|error| search_error("reading semantic vector identity", error))?;
        if !is_sha256(&source_fingerprint)
            || !is_sha256(&input_fingerprint)
            || stored_encoder_fingerprint != encoder_fingerprint
        {
            return Err(AppError::operational(format!(
                "error: semantic vector identity for commit {oid} is invalid or stale; rerun `gitscry index --semantic` while online"
            )));
        }
        let vector = decode_semantic_vector(&embedding, &oid)?;
        let cosine = query_vectors
            .iter()
            .map(|query| cosine_similarity(query, &vector))
            .fold(f64::NEG_INFINITY, f64::max);
        top.insert(SemanticCandidate {
            commit_id,
            oid,
            commit_time,
            cosine,
        });
        if top.len() > limit {
            top.pop_last();
        }
    }
    Ok(top.into_iter().collect())
}

fn validate_query_vectors(query_vectors: &[Vec<f32>]) -> Result<(), AppError> {
    if query_vectors.is_empty() {
        return Err(AppError::operational(
            "error: semantic query produced no embedding vectors",
        ));
    }
    for vector in query_vectors {
        if !normalized_vector(vector) {
            return Err(AppError::operational(
                "error: semantic query vector has an invalid dimension, value, or normalization",
            ));
        }
    }
    Ok(())
}

fn decode_semantic_vector(bytes: &[u8], oid: &str) -> Result<Vec<f32>, AppError> {
    let expected_bytes = crate::semantic::EMBEDDING_DIMENSION * std::mem::size_of::<f32>();
    let vector = if bytes.len() == expected_bytes {
        bytes
            .as_chunks::<{ std::mem::size_of::<f32>() }>()
            .0
            .iter()
            .map(|chunk| f32::from_le_bytes(*chunk))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if !normalized_vector(&vector) {
        return Err(AppError::operational(format!(
            "error: semantic vector for commit {oid} has an invalid dimension, value, or normalization; rerun `gitscry index --semantic` while online"
        )));
    }
    Ok(vector)
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f64 {
    let (dot, left_norm, right_norm) = left.iter().zip(right).fold(
        (0.0, 0.0, 0.0),
        |(dot, left_norm, right_norm), (left, right)| {
            let left = f64::from(*left);
            let right = f64::from(*right);
            (
                dot + left * right,
                left_norm + left * left,
                right_norm + right * right,
            )
        },
    );
    (dot / (left_norm.sqrt() * right_norm.sqrt())).clamp(-1.0, 1.0)
}

fn normalized_vector(vector: &[f32]) -> bool {
    if vector.len() != crate::semantic::EMBEDDING_DIMENSION
        || vector.iter().any(|value| !value.is_finite())
    {
        return false;
    }
    let norm = vector
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        .sqrt();
    norm.is_finite() && (norm - 1.0).abs() <= VECTOR_NORMALIZATION_TOLERANCE
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn semantic_materials(
    connection: &Connection,
    commit_ids: &[i64],
) -> Result<Vec<SearchMaterial>, AppError> {
    if commit_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut ids = commit_ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let mut candidates = Vec::with_capacity(ids.len());
    for ids in ids.chunks(SQL_PARAMETER_LIMIT) {
        let placeholders = numbered_placeholders(1, ids.len());
        let sql = format!(
            "SELECT commit_id, oid, commit_time, message, message_length, position
             FROM commits
             WHERE commit_id IN ({placeholders})
             ORDER BY oid"
        );
        let values = ids.iter().copied().map(Value::Integer);
        let mut statement = connection
            .prepare(&sql)
            .map_err(|error| search_error("preparing semantic result materials", error))?;
        let rows = statement
            .query_map(params_from_iter(values), |row| {
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
                    bm25: 0.0,
                    position: row.get(5)?,
                })
            })
            .map_err(|error| search_error("reading semantic result materials", error))?;
        candidates.extend(
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| search_error("reading semantic result materials", error))?,
        );
    }
    if candidates.len() != ids.len()
        || candidates
            .iter()
            .any(|candidate| ids.binary_search(&candidate.commit_id).is_err())
    {
        return Err(AppError::operational(
            "error: semantic result no longer maps to cache commit materials; rerun `gitscry index --semantic`",
        ));
    }
    load_candidate_paths(connection, &mut candidates)?;
    Ok(candidates
        .into_iter()
        .map(|candidate| SearchMaterial {
            commit_id: candidate.commit_id,
            oid: candidate.oid,
            commit_time: candidate.commit_time,
            subject: candidate.subject,
            paths: candidate.paths,
        })
        .collect())
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

/// Commits eligible under the same revision and committer-time bounds as scoped search.
fn commits_scoped(
    connection: &Connection,
    scope: &SearchFilter,
) -> Result<Vec<StoredCommit>, AppError> {
    let query = format!(
        "{SEARCH_SCOPE_CTE}
         SELECT c.position, c.oid, c.message, c.message_length
         FROM commits AS c
         JOIN eligible ON eligible.commit_id = c.commit_id
         ORDER BY c.commit_time, c.oid"
    );
    let mut statement = connection
        .prepare(&query)
        .map_err(|error| search_error("preparing scoped history scan", error))?;
    statement
        .query_map(params_from_iter(scope_values(scope)), |row| {
            Ok(StoredCommit {
                position: row.get(0)?,
                oid: row.get(1)?,
                message: decode_message_row(row, 2, 3)?,
            })
        })
        .map_err(|error| search_error("reading scoped history scan", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| search_error("reading scoped history scan", error))
}

fn commit_oids(connection: &Connection) -> Result<HashSet<String>, AppError> {
    let mut statement = connection
        .prepare("SELECT oid FROM commits")
        .map_err(|error| search_error("preparing cached object ID scan", error))?;
    statement
        .query_map([], |row| row.get(0))
        .map_err(|error| search_error("reading cached object IDs", error))?
        .collect::<Result<HashSet<_>, _>>()
        .map_err(|error| search_error("reading cached object IDs", error))
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
    scope: Option<&SearchFilter>,
) -> Result<bool, AppError> {
    if path_keys.is_empty() || from_position >= to_position {
        return Ok(false);
    }
    let scoped = scope.is_some();
    let path_chunk_limit = SQL_PARAMETER_LIMIT - if scoped { 6 } else { 2 };
    for path_chunk in path_keys.chunks(path_chunk_limit) {
        let path_placeholders = numbered_placeholders(if scoped { 7 } else { 3 }, path_chunk.len());
        let (query, values) = if let Some(scope) = scope {
            (
                format!(
                    "{SEARCH_SCOPE_CTE}
                     SELECT EXISTS(SELECT 1 FROM commits AS c
                     JOIN eligible ON eligible.commit_id = c.commit_id
                     JOIN commit_paths AS cp ON cp.commit_id = c.commit_id
                     WHERE c.position > ?5 AND c.position < ?6
                       AND cp.path_key IN ({path_placeholders}))"
                ),
                scope_values(scope)
                    .into_iter()
                    .chain([Value::Integer(from_position), Value::Integer(to_position)])
                    .chain(path_chunk.iter().cloned().map(Value::Text))
                    .collect::<Vec<_>>(),
            )
        } else {
            (
                format!(
                    "SELECT EXISTS(SELECT 1 FROM commits AS c
                     JOIN commit_paths AS cp ON cp.commit_id = c.commit_id
                     WHERE c.position > ?1 AND c.position < ?2
                       AND cp.path_key IN ({path_placeholders}))"
                ),
                [Value::Integer(from_position), Value::Integer(to_position)]
                    .into_iter()
                    .chain(path_chunk.iter().cloned().map(Value::Text))
                    .collect::<Vec<_>>(),
            )
        };
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
    scope: Option<&SearchFilter>,
) -> Result<Vec<StoredCommit>, AppError> {
    if path_keys.is_empty() {
        return Ok(Vec::new());
    }
    let scoped = scope.is_some();
    let path_chunk_limit = SQL_PARAMETER_LIMIT - if scoped { 5 } else { 1 };
    let mut candidates = Vec::<(i64, String, Vec<u8>)>::new();
    let mut seen = HashSet::new();
    for path_chunk in path_keys.chunks(path_chunk_limit) {
        let path_placeholders = numbered_placeholders(if scoped { 6 } else { 2 }, path_chunk.len());
        let (query, values) = if let Some(scope) = scope {
            (
                format!(
                    "{SEARCH_SCOPE_CTE}
                     SELECT c.position, c.oid, c.message, c.message_length
                     FROM commits AS c
                     JOIN eligible ON eligible.commit_id = c.commit_id
                     JOIN commit_paths AS cp ON cp.commit_id = c.commit_id
                     WHERE c.position > (SELECT position FROM commits WHERE oid = ?5)
                       AND c.oid <> ?5
                       AND cp.path_key IN ({path_placeholders})
                     GROUP BY c.commit_id
                     ORDER BY c.position ASC"
                ),
                scope_values(scope)
                    .into_iter()
                    .chain(std::iter::once(Value::Text(revert_oid.to_owned())))
                    .chain(path_chunk.iter().cloned().map(Value::Text))
                    .collect::<Vec<_>>(),
            )
        } else {
            (
                format!(
                    "SELECT c.position, c.oid, c.message, c.message_length
                     FROM commits AS c
                     JOIN commit_paths AS cp ON cp.commit_id = c.commit_id
                     WHERE c.position > (SELECT position FROM commits WHERE oid = ?1)
                       AND c.oid <> ?1
                       AND cp.path_key IN ({path_placeholders})
                     GROUP BY c.commit_id
                     ORDER BY c.position ASC"
                ),
                std::iter::once(Value::Text(revert_oid.to_owned()))
                    .chain(path_chunk.iter().cloned().map(Value::Text))
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
        .map(|(position, oid, message)| StoredCommit {
            position,
            oid,
            message,
        })
        .collect())
}

#[cfg(test)]
mod semantic_tests {
    use rusqlite::{Connection, params};

    use super::{SearchFilter, semantic_top_k};

    fn database() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE commits (
                    commit_id INTEGER PRIMARY KEY,
                    oid TEXT NOT NULL,
                    commit_time INTEGER NOT NULL
                );
                CREATE TABLE commit_parents (
                    commit_id INTEGER NOT NULL,
                    parent_id INTEGER
                );
                CREATE TABLE semantic_vectors (
                    commit_id INTEGER PRIMARY KEY,
                    embedding BLOB NOT NULL,
                    source_fingerprint TEXT NOT NULL,
                    input_fingerprint TEXT NOT NULL,
                    encoder_fingerprint TEXT NOT NULL
                );",
            )
            .unwrap();
        connection
    }

    fn unit_vector(dimension: usize) -> Vec<f32> {
        let mut vector = vec![0.0; crate::semantic::EMBEDDING_DIMENSION];
        vector[dimension] = 1.0;
        vector
    }

    fn insert_vector(connection: &Connection, id: i64, oid: &str, vector: &[f32]) {
        let bytes = vector
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>();
        connection
            .execute(
                "INSERT INTO commits (commit_id, oid, commit_time) VALUES (?1, ?2, 1)",
                params![id, oid],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO semantic_vectors (
                    commit_id, embedding, source_fingerprint,
                    input_fingerprint, encoder_fingerprint
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    id,
                    bytes,
                    "a".repeat(64),
                    "b".repeat(64),
                    crate::semantic::encoder_fingerprint()
                ],
            )
            .unwrap();
    }

    #[test]
    fn semantic_top_k_uses_best_query_chunk_and_ties_by_oid() {
        let connection = database();
        let first = unit_vector(0);
        let second = unit_vector(1);
        let mut diagonal = vec![0.0; crate::semantic::EMBEDDING_DIMENSION];
        diagonal[0] = std::f32::consts::FRAC_1_SQRT_2;
        diagonal[1] = std::f32::consts::FRAC_1_SQRT_2;
        insert_vector(&connection, 1, "b", &first);
        insert_vector(&connection, 2, "a", &second);
        insert_vector(&connection, 3, "c", &diagonal);

        let results = semantic_top_k(&connection, &[first, second], 2, None).unwrap();
        assert_eq!(
            results
                .iter()
                .map(|hit| hit.oid.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(results[0].cosine, 1.0);
        assert_eq!(results[0].commit_id, 2);
    }

    #[test]
    fn semantic_scope_filters_vectors_before_top_k() {
        let connection = database();
        let target = unit_vector(0);
        let outside = unit_vector(1);
        insert_vector(&connection, 1, "target", &target);
        insert_vector(&connection, 2, "outside", &outside);
        let scope = SearchFilter {
            from_oid: None,
            to_oid: "target".to_owned(),
            since: None,
            until: None,
        };

        let results = semantic_top_k(&connection, &[outside], 1, Some(&scope)).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].oid, "target");
        assert_eq!(results[0].cosine, 0.0);
    }

    #[test]
    fn semantic_top_k_supports_requested_depth_above_one_hundred() {
        let connection = database();
        let query = unit_vector(0);
        for id in 0..105 {
            insert_vector(&connection, id, &format!("{id:040x}"), &query);
        }

        let results = semantic_top_k(&connection, &[query], 101, None).unwrap();
        assert_eq!(results.len(), 101);
        assert_eq!(results[0].oid, format!("{:040x}", 0));
        assert_eq!(results[100].oid, format!("{:040x}", 100));
    }

    #[test]
    fn semantic_top_k_rejects_corrupt_vector_dimensions() {
        let connection = database();
        let query = unit_vector(0);
        insert_vector(&connection, 1, "a", &query);
        connection
            .execute("UPDATE semantic_vectors SET embedding = X'00'", [])
            .unwrap();

        let error = semantic_top_k(&connection, &[query], 1, None).unwrap_err();
        assert!(error.to_string().contains("invalid dimension"));
    }
}
