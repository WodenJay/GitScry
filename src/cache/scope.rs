//! Cache-side expression of historical query scope, shared by query readers.
use rusqlite::{Connection, params, types::Value};

use crate::{app::AppError, cache::QuerySession};

#[derive(Clone, Debug)]
pub(crate) struct SearchFilter {
    pub(crate) from_oid: Option<String>,
    pub(crate) to_oid: String,
    pub(crate) since: Option<i64>,
    pub(crate) until: Option<i64>,
    /// QUERY-mode historical changed-path eligibility; empty means no path
    /// restriction. Paths are normalized, case-sensitive, and literal.
    pub(crate) paths: Vec<String>,
}

pub(crate) fn set_scope_revisions(
    connection: &Connection,
    reachable_oids: &[String],
    excluded_oids: &[String],
    target_oids: &[String],
) -> Result<(), AppError> {
    connection
        .execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS query_scope_revisions (
                role TEXT NOT NULL,
                oid TEXT NOT NULL,
                PRIMARY KEY (role, oid)
            ) WITHOUT ROWID;
            DELETE FROM temp.query_scope_revisions;",
        )
        .map_err(|error| super::query_error(format!("preparing query scope: {error}")))?;
    let mut statement = connection
        .prepare("INSERT OR IGNORE INTO temp.query_scope_revisions (role, oid) VALUES (?1, ?2)")
        .map_err(|error| super::query_error(format!("preparing query scope: {error}")))?;
    for (role, oids) in [
        ("reachable", reachable_oids),
        ("excluded", excluded_oids),
        ("target", target_oids),
    ] {
        for oid in oids {
            statement
                .execute(params![role, oid])
                .map_err(|error| super::query_error(format!("setting query scope: {error}")))?;
        }
    }
    Ok(())
}

impl QuerySession {
    pub(crate) fn set_scope_revisions(
        &self,
        reachable_oids: &[String],
        excluded_oids: &[String],
        target_oids: &[String],
    ) -> Result<(), AppError> {
        set_scope_revisions(&self.connection, reachable_oids, excluded_oids, target_oids)
    }
}

pub(crate) const SEARCH_SCOPE_CTE: &str = r#"
WITH eligible(commit_id) AS (
    SELECT commits.commit_id
    FROM commits
    WHERE (commits.oid = ?1 OR EXISTS (
        SELECT 1
        FROM temp.query_scope_revisions AS reachable
        WHERE reachable.role = 'reachable' AND reachable.oid = commits.oid
    ))
      AND (?2 IS NULL OR (
          commits.oid <> ?2
          AND NOT EXISTS (
              SELECT 1
              FROM temp.query_scope_revisions AS excluded
              WHERE excluded.role = 'excluded' AND excluded.oid = commits.oid
          )
      ))
      AND (?3 IS NULL OR commits.commit_time >= ?3)
      AND (?4 IS NULL OR commits.commit_time <= ?4)
)
"#;

pub(super) fn scope_values(scope: &SearchFilter) -> [Value; 4] {
    [
        Value::Text(scope.to_oid.clone()),
        scope.from_oid.clone().map_or(Value::Null, Value::Text),
        scope.since.map_or(Value::Null, Value::Integer),
        scope.until.map_or(Value::Null, Value::Integer),
    ]
}

/// SQL predicate restricting a commit to the filter's historical changed paths.
///
/// Returns an empty string when the filter carries no path restriction. The
/// paths are literal: exact equality or a `/`-separated descendant, never a
/// SQL LIKE pattern, never case-folded, never matched by basename.
pub(crate) fn changed_path_predicate(scope: &SearchFilter, first_parameter: usize) -> String {
    // The root selection (`.`) restricts nothing, so it contributes no
    // predicate and no parameters.
    let paths: Vec<&String> = scope
        .paths
        .iter()
        .filter(|path| path.as_str() != ".")
        .collect();
    if paths.is_empty() {
        return String::new();
    }
    let matches = paths
        .iter()
        .enumerate()
        .map(|(index, _path)| {
            let exact = format!("?{}", first_parameter + index * 2);
            let prefix = format!("?{}", first_parameter + index * 2 + 1);
            // `raw_path` is a BLOB, so the TEXT parameters must be cast to
            // blobs; a TEXT value never equals a BLOB column directly.
            format!(
                "(cp.raw_path = CAST({exact} AS BLOB) OR instr(cp.raw_path, CAST({prefix} AS BLOB)) = 1)"
            )
        })
        .collect::<Vec<_>>()
        .join(" OR ");
    format!(
        "AND EXISTS (SELECT 1 FROM commit_paths AS cp \
         WHERE cp.commit_id = c.commit_id AND ({matches}))"
    )
}

/// Parameter values for [`changed_path_predicate`]: one exact value and one
/// `path/` prefix value per non-root requested path, in the same order.
pub(crate) fn changed_path_values(scope: &SearchFilter) -> Vec<Value> {
    scope
        .paths
        .iter()
        .filter(|path| path.as_str() != ".")
        .flat_map(|path| [Value::Text(path.clone()), Value::Text(format!("{path}/"))])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{SearchFilter, changed_path_predicate, changed_path_values};

    fn filter(paths: &[&str]) -> SearchFilter {
        SearchFilter {
            from_oid: None,
            to_oid: "t".to_owned(),
            since: None,
            until: None,
            paths: paths.iter().map(|path| (*path).to_owned()).collect(),
        }
    }

    #[test]
    fn root_scope_contributes_no_predicate_or_parameters() {
        let scope = filter(&["."]);
        assert_eq!(changed_path_predicate(&scope, 5), "");
        assert!(changed_path_values(&scope).is_empty());
        // Root mixed with a subtree keeps only the subtree's parameters.
        let mixed = filter(&[".", "packages/a"]);
        let predicate = changed_path_predicate(&mixed, 5);
        assert!(predicate.contains("CAST(?5 AS BLOB)"), "{predicate}");
        assert_eq!(changed_path_values(&mixed).len(), 2);
    }

    #[test]
    fn subtree_scope_uses_exact_and_prefix_parameters() {
        let scope = filter(&["packages/a"]);
        let predicate = changed_path_predicate(&scope, 5);
        assert!(predicate.contains("CAST(?5 AS BLOB)"), "{predicate}");
        assert!(predicate.contains("CAST(?6 AS BLOB)"), "{predicate}");
        assert_eq!(changed_path_values(&scope).len(), 2);
    }
}
