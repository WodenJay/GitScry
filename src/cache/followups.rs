//! Metadata-only forward graph; changed paths are read only for bounded inspection.

use super::{QuerySession, history::PathChange, message_parts, query_error};
use crate::app::AppError;

pub(crate) struct ForwardCommit {
    pub(crate) oid: String,
    pub(crate) commit_time: i64,
    pub(crate) parents: Vec<String>,
}

impl QuerySession {
    pub(crate) fn forward_graph(&self, endpoint: &str) -> Result<Vec<ForwardCommit>, AppError> {
        let reachable = self.ancestors(endpoint)?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT c.oid, c.commit_time, COALESCE(parent.oid, p.external_oid)
             FROM commits c LEFT JOIN commit_parents p ON p.commit_id = c.commit_id
             LEFT JOIN commits parent ON parent.commit_id = p.parent_id
             ORDER BY c.position, c.oid, p.position",
            )
            .map_err(|e| query_error(format!("reading forward graph: {e}")))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(|e| query_error(format!("reading forward graph: {e}")))?;
        let mut graph: Vec<ForwardCommit> = Vec::new();
        for row in rows {
            let (oid, commit_time, parent) =
                row.map_err(|e| query_error(format!("reading forward graph: {e}")))?;
            if !reachable.contains(&oid) {
                continue;
            }
            if graph.last().is_none_or(|node| node.oid != oid) {
                graph.push(ForwardCommit {
                    oid,
                    commit_time,
                    parents: Vec::new(),
                });
            }
            if let Some(parent) = parent {
                graph
                    .last_mut()
                    .expect("node inserted")
                    .parents
                    .push(parent);
            }
        }
        Ok(graph)
    }

    pub(crate) fn forward_changes(&self, oid: &str) -> Result<Vec<PathChange>, AppError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT ordinal, status, old_path, new_path, old_blob, new_blob FROM changes
             WHERE commit_id = (SELECT commit_id FROM commits WHERE oid = ?1) ORDER BY ordinal",
            )
            .map_err(|e| query_error(format!("reading forward changes: {e}")))?;
        statement
            .query_map([oid], |row| {
                Ok(PathChange {
                    ordinal: row.get(0)?,
                    status: row.get(1)?,
                    old_path: row.get(2)?,
                    new_path: row.get(3)?,
                    old_blob: row.get(4)?,
                    new_blob: row.get(5)?,
                })
            })
            .map_err(|e| query_error(format!("reading forward changes: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| query_error(format!("reading forward changes: {e}")))
    }

    pub(crate) fn forward_subject(&self, oid: &str) -> Result<String, AppError> {
        self.connection
            .query_row(
                "SELECT message, message_length FROM commits WHERE oid = ?1",
                [oid],
                |row| {
                    let message = super::decode_message_row(row, 0, 1)?;
                    Ok(message_parts(&message).0)
                },
            )
            .map_err(|e| query_error(format!("reading forward subject: {e}")))
    }
}
