//! Cache-only incarnation traversal; merge edges remain even when touches do not.
use super::QuerySession;
use crate::app::AppError;
use std::collections::{HashMap, HashSet};
fn cache_error(error: rusqlite::Error) -> AppError {
    super::cache_error("reading hotspots history", error)
}

struct Change {
    status: String,
    old: Option<Vec<u8>>,
    new: Option<Vec<u8>>,
}
struct Node {
    time: i64,
    parents: Vec<Option<i64>>,
    changes: Vec<Change>,
}
pub(crate) struct FileTouches {
    pub(crate) path: Vec<u8>,
    pub(crate) times: Vec<i64>,
}
impl QuerySession {
    pub(crate) fn hotspot_touches(
        &self,
        target: &str,
        paths: Vec<Vec<u8>>,
    ) -> Result<Vec<FileTouches>, AppError> {
        let reachable = self.ancestors(target)?;
        let mut nodes = HashMap::new();
        let mut ids = HashMap::new();
        let mut statement = self
            .connection
            .prepare("SELECT commit_id, oid, commit_time FROM commits")
            .map_err(cache_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(cache_error)?;
        for row in rows {
            let (id, oid, time) = row.map_err(cache_error)?;
            if reachable.contains(&oid) {
                ids.insert(oid, id);
                nodes.insert(
                    id,
                    Node {
                        time,
                        parents: vec![],
                        changes: vec![],
                    },
                );
            }
        }
        let mut statement = self
            .connection
            .prepare("SELECT commit_id, parent_id FROM commit_parents ORDER BY commit_id, position")
            .map_err(cache_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
            })
            .map_err(cache_error)?;
        for row in rows {
            let (id, parent) = row.map_err(cache_error)?;
            if let Some(node) = nodes.get_mut(&id) {
                node.parents.push(parent);
            }
        }
        let mut statement = self.connection.prepare("SELECT commit_id, status, old_path, new_path FROM changes ORDER BY commit_id, ordinal").map_err(cache_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    Change {
                        status: row.get(1)?,
                        old: row.get(2)?,
                        new: row.get(3)?,
                    },
                ))
            })
            .map_err(cache_error)?;
        for row in rows {
            let (id, change) = row.map_err(cache_error)?;
            if let Some(node) = nodes.get_mut(&id) {
                node.changes.push(change);
            }
        }
        let target_id = ids[target];
        Ok(paths
            .into_iter()
            .map(|path| {
                let mut pending = vec![(target_id, path.clone())];
                let mut visited = HashSet::new();
                let mut touches = HashSet::new();
                while let Some((id, alias)) = pending.pop() {
                    if !visited.insert((id, alias.clone())) {
                        continue;
                    }
                    let Some(node) = nodes.get(&id) else {
                        continue;
                    };
                    let change = node
                        .changes
                        .iter()
                        .find(|change| change.new.as_deref() == Some(alias.as_slice()));
                    if node.parents.len() <= 1 && change.is_some() {
                        touches.insert(id);
                    }
                    let previous = match change {
                        Some(change) if change.status.starts_with(['A', 'C']) => None,
                        Some(change) if change.status.starts_with('R') => change.old.as_ref(),
                        _ => Some(&alias),
                    };
                    for (position, parent) in node.parents.iter().enumerate() {
                        let Some(parent) = parent else {
                            continue;
                        };
                        let parent_path = if position == 0 {
                            previous
                        } else if present(&nodes, *parent, &alias) {
                            Some(&alias)
                        } else {
                            previous.filter(|path| present(&nodes, *parent, path))
                        };
                        if let Some(path) = parent_path {
                            pending.push((*parent, path.clone()));
                        }
                    }
                }
                FileTouches {
                    path,
                    times: touches.into_iter().map(|id| nodes[&id].time).collect(),
                }
            })
            .collect())
    }
}

// Reconstruct membership from first-parent cached deltas, without reading historic trees.
fn present(nodes: &HashMap<i64, Node>, mut id: i64, path: &[u8]) -> bool {
    loop {
        let Some(node) = nodes.get(&id) else {
            return false;
        };
        if node
            .changes
            .iter()
            .any(|change| change.new.as_deref() == Some(path))
        {
            return true;
        }
        if node.changes.iter().any(|change| {
            change.old.as_deref() == Some(path) && change.status.starts_with(['D', 'R'])
        }) {
            return false;
        }
        match node.parents.first().copied().flatten() {
            Some(parent) => id = parent,
            None => return false,
        }
    }
}
