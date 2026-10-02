use rusqlite::{Connection, OptionalExtension, params_from_iter, types::Value};
use std::collections::{HashMap, HashSet};

use crate::app::AppError;

use super::scope::{SEARCH_SCOPE_CTE, SearchFilter, scope_values};
use super::semantic::{SemanticCandidate, semantic_top_k};
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
    pub(crate) bm25: f64,
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
    pub(crate) total_touches: usize,
    pub(crate) supporting: Vec<RelationSupport>,
    pub(crate) seed_keys: HashSet<Vec<u8>>,
}

pub(crate) struct RelationHistory {
    pub(crate) candidates: HashMap<Vec<u8>, RelationCandidate>,
    pub(crate) seed_touch_commits: usize,
    pub(crate) eligible_commits: usize,
    pub(crate) mass_changes_filtered: bool,
}

#[derive(Default)]
struct SeedMatches {
    keys: HashSet<Vec<u8>>,
    paths: HashSet<Vec<u8>>,
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
        let seeds = seed_keys
            .iter()
            .map(|key| key.as_bytes().to_vec())
            .collect::<Vec<_>>();
        relation_history(
            &self.connection,
            &seeds,
            mass_change_path_limit,
            scope,
            false,
        )
    }

    pub(crate) fn exact_relation_history(
        &self,
        paths: &[Vec<u8>],
        mass_change_path_limit: usize,
        scope: Option<&SearchFilter>,
    ) -> Result<RelationHistory, AppError> {
        relation_history(&self.connection, paths, mass_change_path_limit, scope, true)
    }

    pub(crate) fn match_count(
        &self,
        match_query: &str,
        explicit_paths: &[String],
    ) -> Result<usize, AppError> {
        match_count(&self.connection, match_query, explicit_paths)
    }

    pub(crate) fn candidates(
        &self,
        match_query: &str,
        limit: i64,
        explicit_paths: &[String],
        complete_terms: &[String],
    ) -> Result<Vec<SearchCandidate>, AppError> {
        candidates(
            &self.connection,
            match_query,
            limit,
            explicit_paths,
            complete_terms,
        )
    }

    pub(crate) fn match_count_scoped(
        &self,
        match_query: &str,
        scope: &SearchFilter,
        explicit_paths: &[String],
    ) -> Result<usize, AppError> {
        match_count_scoped(&self.connection, match_query, scope, explicit_paths)
    }

    pub(crate) fn candidates_scoped(
        &self,
        match_query: &str,
        scope: &SearchFilter,
        explicit_paths: &[String],
    ) -> Result<Vec<SearchCandidate>, AppError> {
        candidates_scoped(&self.connection, match_query, scope, explicit_paths)
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

    pub(crate) fn projected_paths(&self, oid: &str) -> Result<Vec<Vec<u8>>, AppError> {
        projected_paths(&self.connection, oid)
    }

    pub(crate) fn changes(&self, oid: &str) -> Result<Vec<StoredChange>, AppError> {
        changes(&self.connection, oid)
    }

    pub(crate) fn commit_message(&self, oid: &str) -> Result<Option<Vec<u8>>, AppError> {
        commit_message(&self.connection, oid)
    }

    pub(crate) fn pattern_observations(
        &self,
        scope: Option<&SearchFilter>,
    ) -> Result<Vec<PatternObservation>, AppError> {
        let (prefix, predicate, values) = if let Some(scope) = scope {
            (
                format!("{SEARCH_SCOPE_CTE} "),
                "WHERE c.commit_id IN (SELECT commit_id FROM eligible)",
                super::scope::scope_values(scope).to_vec(),
            )
        } else {
            (String::new(), "", Vec::new())
        };
        let sql = format!(
            "{prefix} SELECT c.oid, c.commit_time, (SELECT COUNT(*) FROM commit_parents p WHERE p.commit_id = c.commit_id), cp.raw_path FROM commits c JOIN commit_paths cp ON cp.commit_id = c.commit_id {predicate} ORDER BY c.position, cp.path_order"
        );
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|e| search_error("preparing pattern observations", e))?;
        let rows = statement
            .query_map(params_from_iter(values), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, usize>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            })
            .map_err(|e| search_error("reading pattern observations", e))?;
        let mut observations: Vec<PatternObservation> = Vec::new();
        for row in rows {
            let (oid, commit_time, parent_count, path) =
                row.map_err(|e| search_error("reading pattern observations", e))?;
            if observations
                .last()
                .is_none_or(|observation| observation.oid != oid)
            {
                observations.push(PatternObservation {
                    oid,
                    commit_time,
                    parent_count,
                    paths: std::collections::BTreeSet::new(),
                });
            }
            observations
                .last_mut()
                .expect("observation just inserted")
                .paths
                .insert(path);
        }
        Ok(observations)
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
        path_ids: &[Vec<u8>],
        scope: Option<&SearchFilter>,
    ) -> Result<bool, AppError> {
        touch_between(
            &self.connection,
            from_position,
            to_position,
            path_ids,
            scope,
        )
    }

    pub(crate) fn follow_ups(
        &self,
        revert_oid: &str,
        path_ids: &[Vec<u8>],
        scope: Option<&SearchFilter>,
    ) -> Result<Vec<StoredCommit>, AppError> {
        follow_ups(&self.connection, revert_oid, path_ids, scope)
    }
}

fn search_error(operation: &str, error: impl std::fmt::Display) -> AppError {
    cache_error(operation, error)
}

fn relation_history(
    connection: &Connection,
    seed_keys: &[Vec<u8>],
    mass_change_path_limit: usize,
    scope: Option<&SearchFilter>,
    exact: bool,
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
    let seed_matches = relation_seed_matches(connection, seed_keys, limit, scope, exact)?;
    let seed_touch_commits = seed_matches.len();
    let mass_changes_filtered =
        relation_has_mass_change(connection, seed_keys, limit, scope, exact)?;
    let mut candidates = HashMap::new();
    let seed_commit_ids = seed_matches.keys().copied().collect::<Vec<_>>();
    for commit_ids in seed_commit_ids.chunks(SQL_PARAMETER_LIMIT) {
        let placeholders = numbered_placeholders(1, commit_ids.len());
        let query = format!(
            "SELECT cp.commit_id, cp.raw_path, c.oid, c.commit_time\n\
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
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(|error| search_error("reading relation support lookup", error))?;
        for row in rows {
            let (commit_id, raw_path, oid, commit_time) =
                row.map_err(|error| search_error("reading relation support lookup", error))?;
            let Some(touched_seeds) = seed_matches.get(&commit_id) else {
                continue;
            };
            if touched_seeds.paths.contains(&raw_path) {
                continue;
            }
            let candidate =
                candidates
                    .entry(raw_path.clone())
                    .or_insert_with(|| RelationCandidate {
                        path: raw_path,
                        total_touches: 0,
                        supporting: Vec::new(),
                        seed_keys: HashSet::new(),
                    });
            candidate
                .seed_keys
                .extend(touched_seeds.keys.iter().cloned());
            candidate
                .supporting
                .push(RelationSupport { oid, commit_time });
        }
    }
    for path_ids in candidates
        .keys()
        .cloned()
        .collect::<Vec<_>>()
        .chunks(SQL_PARAMETER_LIMIT)
    {
        let query_bindings = RelationQueryBindings::new(scope, &[], limit, false);
        let scope_cte = &query_bindings.cte_prefix;
        let limit_parameter = query_bindings.limit_parameter;
        let placeholders =
            numbered_placeholders(query_bindings.first_seed_parameter, path_ids.len());
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
             SELECT cp.raw_path,\n\
                    COUNT(*) OVER (PARTITION BY cp.raw_path) AS total_touches,\n\
                    ROW_NUMBER() OVER (\n\
                        PARTITION BY cp.raw_path\n\
                        ORDER BY c.position, cp.path_order\n\
                    ) AS path_rank\n\
             FROM commit_paths AS cp\n\
             JOIN relation_eligible ON relation_eligible.commit_id = cp.commit_id\n\
             JOIN commits AS c ON c.commit_id = cp.commit_id\n\
             WHERE cp.raw_path IN ({placeholders})\n\
             )\n\
             SELECT raw_path, total_touches\n\
             FROM ranked\n\
             WHERE path_rank = 1"
        );
        let mut values = query_bindings.values;
        values.extend(path_ids.iter().cloned().map(Value::Blob));
        let mut statement = connection
            .prepare(&query)
            .map_err(|error| search_error("preparing relation touch lookup", error))?;
        let rows = statement
            .query_map(params_from_iter(values), |row| {
                Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|error| search_error("reading relation touch lookup", error))?;
        for row in rows {
            let (path_id, total_touches) =
                row.map_err(|error| search_error("reading relation touch lookup", error))?;
            if let Some(candidate) = candidates.get_mut(&path_id) {
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
    seed_scope_predicate: &'static str,
    values: Vec<Value>,
}

impl RelationQueryBindings {
    fn new(scope: Option<&SearchFilter>, seed_keys: &[Vec<u8>], limit: i64, exact: bool) -> Self {
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
        let seed_scope_predicate = if scope.is_some() {
            "AND exact.commit_id IN (SELECT commit_id FROM eligible)"
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
            values.push(if exact {
                Value::Blob(seed.clone())
            } else {
                Value::Text(String::from_utf8_lossy(seed).into_owned())
            });
            values.push(Value::Integer(if exact {
                2
            } else {
                (!seed.contains(&b'/')) as i64
            }));
        }

        Self {
            cte_prefix,
            limit_parameter,
            first_seed_parameter,
            scope_predicate,
            seed_scope_predicate,
            values,
        }
    }
}

fn relation_count(
    connection: &Connection,
    limit: i64,
    scope: Option<&SearchFilter>,
) -> Result<i64, AppError> {
    let query_bindings = RelationQueryBindings::new(scope, &[], limit, false);
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
    seed_keys: &[Vec<u8>],
    limit: i64,
    scope: Option<&SearchFilter>,
    exact: bool,
) -> Result<HashMap<i64, SeedMatches>, AppError> {
    let query_bindings = RelationQueryBindings::new(scope, seed_keys, limit, exact);
    let scope_cte = &query_bindings.cte_prefix;
    let limit_parameter = query_bindings.limit_parameter;
    let values_clause = seed_values_clause(seed_keys, query_bindings.first_seed_parameter);
    let scope_predicate = query_bindings.scope_predicate;
    let seed_scope_predicate = query_bindings.seed_scope_predicate;
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
         SELECT DISTINCT cp.commit_id, CAST(seed_keys.seed_key AS BLOB), cp.raw_path\n\
         FROM commit_paths AS cp\n\
         JOIN relation_eligible ON relation_eligible.commit_id = cp.commit_id\n\
         JOIN seed_keys ON (\n\
             seed_keys.is_basename = 2 AND cp.raw_path = seed_keys.seed_key\n\
         ) OR (\n\
             seed_keys.is_basename = 1 AND cp.path_basename = lower(seed_keys.seed_key)\n\
         ) OR (\n\
             seed_keys.is_basename = 0 AND (\n\
                 cp.raw_path = CAST(seed_keys.seed_key AS BLOB)\n\
                 OR (cp.path_search_key = lower(seed_keys.seed_key)\n\
                     AND NOT EXISTS (\n\
                         SELECT 1 FROM commit_paths AS exact\n\
                         WHERE exact.raw_path = CAST(seed_keys.seed_key AS BLOB)\n\
                         {seed_scope_predicate}\n\
                     ))\n\
             )\n\
         )\n\
         ORDER BY cp.commit_id, seed_keys.seed_key, cp.raw_path"
    );
    let mut statement = connection
        .prepare(&query)
        .map_err(|error| search_error("preparing relation seed lookup", error))?;
    let rows = statement
        .query_map(params_from_iter(query_bindings.values), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })
        .map_err(|error| search_error("reading relation seed lookup", error))?;
    let mut matches = HashMap::<i64, SeedMatches>::new();
    for row in rows {
        let (commit_id, seed_key, raw_path) =
            row.map_err(|error| search_error("reading relation seed lookup", error))?;
        let seed = matches.entry(commit_id).or_default();
        seed.keys.insert(seed_key);
        seed.paths.insert(raw_path);
    }
    Ok(matches)
}

fn relation_has_mass_change(
    connection: &Connection,
    seed_keys: &[Vec<u8>],
    limit: i64,
    scope: Option<&SearchFilter>,
    exact: bool,
) -> Result<bool, AppError> {
    let query_bindings = RelationQueryBindings::new(scope, seed_keys, limit, exact);
    let scope_cte = &query_bindings.cte_prefix;
    let limit_parameter = query_bindings.limit_parameter;
    let values_clause = seed_values_clause(seed_keys, query_bindings.first_seed_parameter);
    let scope_predicate = query_bindings.scope_predicate;
    let seed_scope_predicate = query_bindings.seed_scope_predicate;
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
                   JOIN seed_keys ON (\n\
                       seed_keys.is_basename = 2 AND cp.raw_path = seed_keys.seed_key\n\
                   ) OR (\n\
                       seed_keys.is_basename = 1 AND cp.path_basename = lower(seed_keys.seed_key)\n\
                   ) OR (\n\
                       seed_keys.is_basename = 0 AND (\n\
                           cp.raw_path = CAST(seed_keys.seed_key AS BLOB)\n\
                           OR (cp.path_search_key = lower(seed_keys.seed_key)\n\
                               AND NOT EXISTS (\n\
                                   SELECT 1 FROM commit_paths AS exact\n\
                                   WHERE exact.raw_path = CAST(seed_keys.seed_key AS BLOB)\n\
                                   {seed_scope_predicate}\n\
                               ))\n\
                       )\n\
                   )\n\
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

fn seed_values_clause(seed_keys: &[Vec<u8>], first_parameter: usize) -> String {
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

fn match_count(
    connection: &Connection,
    match_query: &str,
    explicit_paths: &[String],
) -> Result<usize, AppError> {
    let path_filter = explicit_path_filter(explicit_paths, 2);
    let query = format!(
        "SELECT COUNT(*)
         FROM search_fts
         JOIN commits AS c ON c.commit_id = search_fts.rowid
         WHERE search_fts MATCH ?1 {path_filter}"
    );
    let mut values = vec![Value::Text(match_query.to_owned())];
    values.extend(explicit_paths.iter().cloned().map(Value::Text));
    let count = connection
        .query_row(&query, params_from_iter(values), |row| row.get::<_, i64>(0))
        .map_err(|error| search_error("counting search matches", error))?;
    usize::try_from(count)
        .map_err(|_| search_error("counting search matches", "count exceeded platform limits"))
}

fn match_count_scoped(
    connection: &Connection,
    match_query: &str,
    scope: &SearchFilter,
    explicit_paths: &[String],
) -> Result<usize, AppError> {
    let path_filter = explicit_path_filter(explicit_paths, 6);
    let query = format!(
        "{SEARCH_SCOPE_CTE}
         SELECT COUNT(*)
         FROM search_fts
         JOIN commits AS c ON c.commit_id = search_fts.rowid
         WHERE search_fts MATCH ?5
           AND c.commit_id IN (SELECT commit_id FROM eligible) {path_filter}"
    );
    let mut values = scope_values(scope).to_vec();
    values.push(Value::Text(match_query.to_owned()));
    values.extend(explicit_paths.iter().cloned().map(Value::Text));
    let count = connection
        .query_row(&query, params_from_iter(values), |row| row.get::<_, i64>(0))
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
    explicit_paths: &[String],
) -> Result<Vec<SearchCandidate>, AppError> {
    // FTS5 BM25 uses corpus-wide statistics, including out-of-scope commits. Search reranks
    // every scoped match with candidate-local signals before applying the result limit.
    let path_filter = explicit_path_filter(explicit_paths, 6);
    let query = format!(
        "{SEARCH_SCOPE_CTE}
         SELECT c.commit_id, c.oid, c.commit_time, c.message, c.message_length,
                0.0, c.position
         FROM search_fts
         JOIN commits AS c ON c.commit_id = search_fts.rowid
         WHERE search_fts MATCH ?5
           AND c.commit_id IN (SELECT commit_id FROM eligible) {path_filter}
         ORDER BY c.commit_time DESC, c.oid ASC"
    );
    let mut values = scope_values(scope).to_vec();
    values.push(Value::Text(match_query.to_owned()));
    values.extend(explicit_paths.iter().cloned().map(Value::Text));
    let mut statement = connection
        .prepare(&query)
        .map_err(|error| search_error("preparing scoped search", error))?;
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
                bm25: row.get(5)?,
                position: row.get(6)?,
            })
        })
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
    explicit_paths: &[String],
    complete_terms: &[String],
) -> Result<Vec<SearchCandidate>, AppError> {
    let path_filter = explicit_path_filter(explicit_paths, 2);
    let complete_filter = complete_cjk_filter(complete_terms, explicit_paths.len() + 2);
    let limit_parameter = explicit_paths.len() + complete_terms.len() + 2;
    let query = format!(
        "SELECT c.commit_id, c.oid, c.commit_time, c.message, c.message_length,
                bm25(search_fts, 10.0, 3.0, 2.0), c.position
         FROM search_fts
         JOIN commits AS c ON c.commit_id = search_fts.rowid
         WHERE search_fts MATCH ?1 {path_filter}
         ORDER BY CASE WHEN ({complete_filter}) THEN 0 ELSE 1 END,
                  bm25(search_fts, 10.0, 3.0, 2.0), c.commit_time DESC, c.oid ASC
         LIMIT ?{limit_parameter}"
    );
    let mut values = vec![Value::Text(match_query.to_owned())];
    values.extend(explicit_paths.iter().cloned().map(Value::Text));
    values.extend(
        complete_terms
            .iter()
            .map(|term| Value::Blob(term.as_bytes().to_vec())),
    );
    values.push(Value::Integer(limit));
    let mut statement = connection
        .prepare(&query)
        .map_err(|error| search_error("preparing search", error))?;
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

fn complete_cjk_filter(terms: &[String], first_parameter: usize) -> String {
    if terms.is_empty() {
        return "0".to_owned();
    }
    terms
        .iter()
        .enumerate()
        .map(|(index, _)| {
            let parameter = first_parameter + index;
            format!(
                "(instr(c.message, ?{parameter}) > 0 OR EXISTS (\
                    SELECT 1 FROM commit_paths AS complete_path \
                    WHERE complete_path.commit_id = c.commit_id \
                      AND instr(complete_path.raw_path, ?{parameter}) > 0))"
            )
        })
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn explicit_path_filter(explicit_paths: &[String], first_parameter: usize) -> String {
    if explicit_paths.is_empty() {
        return String::new();
    }
    let matches = explicit_paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let parameter = format!("?{}", first_parameter + index);
            if path.contains('/') {
                format!("cp.path_search_key = lower({parameter})")
            } else {
                format!("(cp.path_search_key = lower({parameter}) OR cp.path_basename = lower({parameter}))")
            }
        })
        .collect::<Vec<_>>()
        .join(" OR ");
    format!(
        "AND EXISTS (SELECT 1 FROM commit_paths AS cp \
         WHERE cp.commit_id = c.commit_id AND ({matches}))"
    )
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
        for candidate in candidates {
            candidate.paths = paths_by_commit
                .remove(&candidate.commit_id)
                .unwrap_or_default();
        }
    }
    Ok(())
}
fn projected_paths(connection: &Connection, oid: &str) -> Result<Vec<Vec<u8>>, AppError> {
    let mut statement = connection
        .prepare(
            "SELECT cp.raw_path
             FROM commit_paths AS cp
             JOIN commits AS c ON c.commit_id = cp.commit_id
             WHERE c.oid = ?1
             ORDER BY cp.path_order",
        )
        .map_err(|error| search_error("preparing path projection", error))?;
    statement
        .query_map([oid], |row| row.get::<_, Vec<u8>>(0))
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

pub(crate) struct PatternObservation {
    pub(crate) oid: String,
    pub(crate) commit_time: i64,
    pub(crate) parent_count: usize,
    pub(crate) paths: std::collections::BTreeSet<Vec<u8>>,
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
        .prepare("SELECT position, oid, message, message_length FROM commits ORDER BY position")
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
         ORDER BY c.position"
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

/// Whether any commit strictly between two positions touched one of `path_ids`.
///
/// A revert only counts as undoing a candidate when that candidate was the last work on
/// the path; otherwise the revert belongs to some later, unrelated change.
fn touch_between(
    connection: &Connection,
    from_position: i64,
    to_position: i64,
    path_ids: &[Vec<u8>],
    scope: Option<&SearchFilter>,
) -> Result<bool, AppError> {
    if path_ids.is_empty() || from_position >= to_position {
        return Ok(false);
    }
    let scoped = scope.is_some();
    let path_chunk_limit = SQL_PARAMETER_LIMIT - if scoped { 6 } else { 2 };
    for path_chunk in path_ids.chunks(path_chunk_limit) {
        let path_placeholders = numbered_placeholders(if scoped { 7 } else { 3 }, path_chunk.len());
        let (query, values) = if let Some(scope) = scope {
            (
                format!(
                    "{SEARCH_SCOPE_CTE}
                     SELECT EXISTS(SELECT 1 FROM commits AS c
                     JOIN eligible ON eligible.commit_id = c.commit_id
                     JOIN commit_paths AS cp ON cp.commit_id = c.commit_id
                     WHERE c.position > ?5 AND c.position < ?6
                       AND cp.raw_path IN ({path_placeholders}))"
                ),
                scope_values(scope)
                    .into_iter()
                    .chain([Value::Integer(from_position), Value::Integer(to_position)])
                    .chain(path_chunk.iter().cloned().map(Value::Blob))
                    .collect::<Vec<_>>(),
            )
        } else {
            (
                format!(
                    "SELECT EXISTS(SELECT 1 FROM commits AS c
                     JOIN commit_paths AS cp ON cp.commit_id = c.commit_id
                     WHERE c.position > ?1 AND c.position < ?2
                       AND cp.raw_path IN ({path_placeholders}))"
                ),
                [Value::Integer(from_position), Value::Integer(to_position)]
                    .into_iter()
                    .chain(path_chunk.iter().cloned().map(Value::Blob))
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
        .map(|(position, oid, message)| StoredCommit {
            position,
            oid,
            message,
        })
        .collect())
}
