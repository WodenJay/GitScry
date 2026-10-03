//! Cache-side expression of historical query scope, shared by query readers.
use rusqlite::{Connection, params, types::Value};

use crate::{app::AppError, cache::QuerySession};

#[derive(Clone, Debug)]
pub(crate) struct SearchFilter {
    pub(crate) from_oid: Option<String>,
    pub(crate) to_oid: String,
    pub(crate) since: Option<i64>,
    pub(crate) until: Option<i64>,
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
