//! Lexical candidates and semantic candidate material hydration.

use rusqlite::{Connection, params_from_iter, types::Value};
use std::collections::HashMap;

use super::{SQL_PARAMETER_LIMIT, numbered_placeholders, search_error};
use crate::app::AppError;
use crate::cache::scope::{SEARCH_SCOPE_CTE, SearchFilter, scope_values};
use crate::cache::semantic::{SemanticCandidate, semantic_top_k};
use crate::cache::{QuerySession, decode_message_row, message_parts};

impl QuerySession {
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
        crate::cache::semantic::require_ready_for_query(&self.connection)
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
    pub(crate) bm25: f64,
}

pub(crate) struct SearchMaterial {
    pub(crate) commit_id: i64,
    pub(crate) oid: String,
    pub(crate) commit_time: i64,
    pub(crate) subject: String,
    pub(crate) paths: Vec<Vec<u8>>,
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
