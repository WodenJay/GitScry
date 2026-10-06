//! Scoped, deterministic top-k retrieval of compatible semantic vectors.
use super::super::{
    cache_error as search_error,
    scope::{
        SEARCH_SCOPE_CTE, SearchFilter, changed_path_predicate, changed_path_values, scope_values,
    },
};
use super::vectors::{self, normalized_vector, valid_fingerprint};
use crate::app::AppError;
use rusqlite::{Connection, params_from_iter};
use std::{cmp::Ordering, collections::BTreeSet};
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
pub(in crate::cache) fn semantic_top_k(
    connection: &Connection,
    query_vectors: &[Vec<f32>],
    limit: usize,
    scope: Option<&SearchFilter>,
) -> Result<Vec<SemanticCandidate>, AppError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    if query_vectors.is_empty() {
        return Err(AppError::operational(
            "error: semantic query has no chunk embeddings",
        ));
    }
    for query_vector in query_vectors {
        validate_query_vector(query_vector)?;
    }
    let encoder_fingerprint = crate::semantic::encoder_fingerprint();
    let sql = match scope {
        Some(scope) => {
            let eligibility = changed_path_predicate(scope, scope_values(scope).len() + 1);
            format!(
                "{SEARCH_SCOPE_CTE}
                 SELECT c.commit_id, c.oid, c.commit_time, v.commit_oid, v.embedding,
                        v.source_fingerprint, v.input_fingerprint, v.encoder_fingerprint
                 FROM eligible
                 JOIN commits AS c ON c.commit_id = eligible.commit_id
                 JOIN semantic_vectors AS v ON v.commit_id = c.commit_id
                 WHERE 1 = 1 {eligibility}
"
            )
        }
        None => "SELECT c.commit_id, c.oid, c.commit_time, v.commit_oid, v.embedding,
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
        Some(scope) => {
            let mut values = scope_values(scope).to_vec();
            values.extend(changed_path_values(scope));
            statement
                .query(params_from_iter(values))
                .map_err(|error| search_error("running scoped semantic retrieval", error))?
        }
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
        let cached_oid: String = row
            .get(3)
            .map_err(|error| search_error("reading semantic vector identity", error))?;
        let embedding: Vec<u8> = row
            .get(4)
            .map_err(|error| search_error("reading semantic vector", error))?;
        let source_fingerprint: String = row
            .get(5)
            .map_err(|error| search_error("reading semantic vector identity", error))?;
        let input_fingerprint: String = row
            .get(6)
            .map_err(|error| search_error("reading semantic vector identity", error))?;
        let stored_encoder_fingerprint: String = row
            .get(7)
            .map_err(|error| search_error("reading semantic vector identity", error))?;
        if cached_oid != oid
            || !valid_fingerprint(Some(&source_fingerprint))
            || !valid_fingerprint(Some(&input_fingerprint))
            || stored_encoder_fingerprint != encoder_fingerprint
        {
            return Err(AppError::operational(format!(
                "error: semantic vector identity for commit {oid} is invalid or stale; rerun `gitscry index --semantic` while online"
            )));
        }
        let vector = decode_semantic_vector(&embedding, &oid)?;
        let cosine = query_vectors
            .iter()
            .map(|query_vector| cosine_similarity(query_vector, &vector))
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

fn validate_query_vector(query_vector: &[f32]) -> Result<(), AppError> {
    if !normalized_vector(query_vector) {
        return Err(AppError::operational(
            "error: semantic query vector has an invalid dimension, value, or normalization",
        ));
    }
    Ok(())
}

fn decode_semantic_vector(bytes: &[u8], oid: &str) -> Result<Vec<f32>, AppError> {
    vectors::decode(bytes).ok_or_else(|| AppError::operational(format!(
        "error: semantic vector for commit {oid} has an invalid dimension, value, or normalization; rerun `gitscry index --semantic` while online"
    )))
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
                    commit_oid TEXT NOT NULL,
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
                    commit_id, commit_oid, embedding, source_fingerprint,
                    input_fingerprint, encoder_fingerprint
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    id,
                    oid,
                    bytes,
                    "a".repeat(64),
                    "b".repeat(64),
                    crate::semantic::encoder_fingerprint()
                ],
            )
            .unwrap();
    }

    #[test]
    fn semantic_top_k_breaks_equal_scores_by_oid() {
        let connection = database();
        let query = unit_vector(0);
        insert_vector(&connection, 1, "b", &query);
        insert_vector(&connection, 2, "a", &query);

        let results = semantic_top_k(&connection, std::slice::from_ref(&query), 2, None).unwrap();
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
    fn semantic_top_k_uses_each_commits_best_chunk_score() {
        let connection = database();
        let head = unit_vector(0);
        let middle = unit_vector(1);
        let tail = unit_vector(2);
        let mut between = vec![0.0; crate::semantic::EMBEDDING_DIMENSION];
        between[0] = 0.8;
        between[1] = 0.6;
        let miss = unit_vector(3);
        insert_vector(&connection, 1, "tail", &tail);
        insert_vector(&connection, 2, "between", &between);
        insert_vector(&connection, 3, "head", &head);
        insert_vector(&connection, 4, "miss", &miss);
        insert_vector(&connection, 5, "middle", &middle);

        let query_chunks = [head, middle, tail];
        let results = semantic_top_k(&connection, &query_chunks, 5, None).unwrap();

        assert_eq!(
            results
                .iter()
                .map(|hit| hit.oid.as_str())
                .collect::<Vec<_>>(),
            ["head", "middle", "tail", "between", "miss"]
        );
        assert_eq!(results[0].cosine, 1.0);
        assert_eq!(results[1].cosine, 1.0);
        assert_eq!(results[2].cosine, 1.0);
        assert!((results[3].cosine - 0.8).abs() < 1e-6);
        assert_eq!(results[4].cosine, 0.0);
    }

    #[test]
    fn semantic_scope_filters_vectors_before_top_k() {
        let connection = database();
        let target = unit_vector(0);
        let outside = unit_vector(1);
        insert_vector(&connection, 1, "target", &target);
        insert_vector(&connection, 2, "outside", &outside);
        crate::cache::scope::set_scope_revisions(
            &connection,
            &["target".to_owned()],
            &[],
            &["target".to_owned()],
        )
        .unwrap();
        let scope = SearchFilter {
            from_oid: None,
            to_oid: "target".to_owned(),
            since: None,
            until: None,
            paths: Vec::new(),
        };

        let results =
            semantic_top_k(&connection, std::slice::from_ref(&outside), 1, Some(&scope)).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].oid, "target");
        assert_eq!(results[0].cosine, 0.0);
    }

    #[test]
    fn semantic_top_k_applies_changed_path_scope_before_ranking() {
        let connection = database();
        connection
            .execute_batch(
                "CREATE TABLE commit_paths (
                    commit_id INTEGER NOT NULL,
                    path_search_key TEXT NOT NULL,
                    path_basename TEXT NOT NULL,
                    raw_path BLOB NOT NULL,
                    path_order INTEGER NOT NULL,
                    PRIMARY KEY (commit_id, raw_path)
                );",
            )
            .unwrap();
        crate::cache::scope::set_scope_revisions(
            &connection,
            &["in-scope".to_owned(), "out-of-scope".to_owned()],
            &[],
            &["in-scope".to_owned(), "out-of-scope".to_owned()],
        )
        .unwrap();
        let query = unit_vector(0);
        insert_vector(&connection, 1, "in-scope", &query);
        insert_vector(&connection, 2, "out-of-scope", &query);
        for (commit_id, path) in [(1, "packages/a/engine.txt"), (2, "packages/b/core.txt")] {
            connection
                .execute(
                    "INSERT INTO commit_paths (commit_id, path_search_key, path_basename, raw_path, path_order)
                     VALUES (?1, ?2, ?3, ?4, 0)",
                    params![commit_id, path, path.rsplit('/').next().unwrap(), path],
                )
                .unwrap();
        }
        let scope = SearchFilter {
            from_oid: None,
            to_oid: "out-of-scope".to_owned(),
            since: None,
            until: None,
            paths: vec!["packages/a".to_owned()],
        };

        // Both commits carry identical vectors; path eligibility decides alone.
        let results =
            semantic_top_k(&connection, std::slice::from_ref(&query), 2, Some(&scope)).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].oid, "in-scope");
    }

    #[test]
    fn semantic_top_k_supports_requested_depth_above_one_hundred() {
        let connection = database();
        let query = unit_vector(0);
        for id in 0..105 {
            insert_vector(&connection, id, &format!("{id:040x}"), &query);
        }

        let results = semantic_top_k(&connection, std::slice::from_ref(&query), 101, None).unwrap();
        assert_eq!(results.len(), 101);
        assert_eq!(results[0].oid, format!("{:040x}", 0));
        assert_eq!(results[100].oid, format!("{:040x}", 100));
    }

    #[test]
    fn semantic_top_k_rejects_a_vector_for_a_different_commit_oid() {
        let connection = database();
        let query = unit_vector(0);
        insert_vector(&connection, 1, "current", &query);
        connection
            .execute("UPDATE semantic_vectors SET commit_oid = 'stale'", [])
            .unwrap();

        let error = semantic_top_k(&connection, std::slice::from_ref(&query), 1, None).unwrap_err();
        assert!(error.to_string().contains("invalid or stale"));
    }

    #[test]
    fn semantic_top_k_rejects_corrupt_vector_dimensions() {
        let connection = database();
        let query = unit_vector(0);
        insert_vector(&connection, 1, "a", &query);
        connection
            .execute("UPDATE semantic_vectors SET embedding = X'00'", [])
            .unwrap();

        let error = semantic_top_k(&connection, std::slice::from_ref(&query), 1, None).unwrap_err();
        assert!(error.to_string().contains("invalid dimension"));
    }
    #[test]
    fn semantic_top_k_rejects_invalid_query_and_stored_values() {
        let connection = database();
        let query = unit_vector(0);
        insert_vector(&connection, 1, "a", &query);
        for value in [0.0, 2.0, f32::NAN, f32::INFINITY] {
            let mut invalid = query.clone();
            invalid[0] = value;
            let error = semantic_top_k(&connection, &[invalid.clone()], 1, None).unwrap_err();
            assert!(error.to_string().contains("semantic query vector"));
            let bytes = invalid
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect::<Vec<_>>();
            connection
                .execute("UPDATE semantic_vectors SET embedding = ?1", [bytes])
                .unwrap();
            let error =
                semantic_top_k(&connection, std::slice::from_ref(&query), 1, None).unwrap_err();
            assert!(error.to_string().contains("semantic vector for commit a"));
        }
    }
}
