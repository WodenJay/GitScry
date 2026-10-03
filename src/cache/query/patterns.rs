//! Cached change-pattern observations.

use rusqlite::params_from_iter;

use super::search_error;
use crate::app::AppError;
use crate::cache::QuerySession;
use crate::cache::scope::{SEARCH_SCOPE_CTE, SearchFilter, scope_values};

impl QuerySession {
    pub(crate) fn pattern_observations(
        &self,
        scope: Option<&SearchFilter>,
    ) -> Result<Vec<PatternObservation>, AppError> {
        let (prefix, predicate, values) = if let Some(scope) = scope {
            (
                format!("{SEARCH_SCOPE_CTE} "),
                "WHERE c.commit_id IN (SELECT commit_id FROM eligible)",
                scope_values(scope).to_vec(),
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
}
pub(crate) struct PatternObservation {
    pub(crate) oid: String,
    pub(crate) commit_time: i64,
    pub(crate) parent_count: usize,
    pub(crate) paths: std::collections::BTreeSet<Vec<u8>>,
}
