//! Co-change evidence, seed matching and mass-change exclusions.

use rusqlite::{Connection, params_from_iter, types::Value};
use std::collections::{HashMap, HashSet};

use super::{SQL_PARAMETER_LIMIT, numbered_placeholders, search_error};
use crate::app::AppError;
use crate::cache::QuerySession;
use crate::cache::scope::{SEARCH_SCOPE_CTE, SearchFilter};

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
            None,
            false,
        )
    }

    pub(crate) fn selected_relation_history(
        &self,
        paths: &[Vec<u8>],
        commits: &HashSet<String>,
        mass_change_path_limit: usize,
        scope: Option<&SearchFilter>,
        include_single_path: bool,
    ) -> Result<RelationHistory, AppError> {
        relation_history(
            &self.connection,
            paths,
            mass_change_path_limit,
            scope,
            true,
            Some(commits),
            include_single_path,
        )
    }
    pub(crate) fn exact_relation_history(
        &self,
        paths: &[Vec<u8>],
        mass_change_path_limit: usize,
        scope: Option<&SearchFilter>,
    ) -> Result<RelationHistory, AppError> {
        relation_history(
            &self.connection,
            paths,
            mass_change_path_limit,
            scope,
            true,
            None,
            false,
        )
    }
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

fn relation_history(
    connection: &Connection,
    seed_keys: &[Vec<u8>],
    mass_change_path_limit: usize,
    scope: Option<&SearchFilter>,
    exact: bool,
    commits: Option<&HashSet<String>>,
    include_single_path: bool,
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
    let mut seed_matches = relation_seed_matches(
        connection,
        seed_keys,
        limit,
        scope,
        exact,
        include_single_path,
    )?;
    if let Some(commits) = commits {
        let mut statement = connection
            .prepare("SELECT commit_id, oid FROM commits")
            .map_err(|error| search_error("preparing selected relation history", error))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| search_error("reading selected relation history", error))?;
        let mut selected = HashSet::new();
        for row in rows {
            let (id, oid) =
                row.map_err(|error| search_error("reading selected relation history", error))?;
            if commits.contains(&oid) {
                selected.insert(id);
            }
        }
        seed_matches.retain(|id, _| selected.contains(id));
    }
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
    include_single_path: bool,
) -> Result<HashMap<i64, SeedMatches>, AppError> {
    let query_bindings = RelationQueryBindings::new(scope, seed_keys, limit, exact);
    let scope_cte = &query_bindings.cte_prefix;
    let limit_parameter = query_bindings.limit_parameter;
    let values_clause = seed_values_clause(seed_keys, query_bindings.first_seed_parameter);
    let scope_predicate = query_bindings.scope_predicate;
    let seed_scope_predicate = query_bindings.seed_scope_predicate;
    let minimum_paths = if include_single_path { 0 } else { 1 };
    let query = format!(
        "{scope_cte}\n\
         relation_eligible AS (\n\
             SELECT pc.commit_id\n\
             FROM commit_path_counts AS pc\n\
             WHERE pc.path_count > {minimum_paths} AND pc.path_count <= ?{limit_parameter}\n\
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
