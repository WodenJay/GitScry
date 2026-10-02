//! Cache-side expression of historical query scope, shared by query readers.
use rusqlite::types::Value;
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

pub(super) fn scope_values(scope: &SearchFilter) -> [Value; 4] {
    [
        Value::Text(scope.to_oid.clone()),
        scope.from_oid.clone().map_or(Value::Null, Value::Text),
        scope.since.map_or(Value::Null, Value::Integer),
        scope.until.map_or(Value::Null, Value::Integer),
    ]
}
