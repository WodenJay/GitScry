//! Cache-only incarnation traversal; merge edges remain even when touches do not.
use super::QuerySession;
use crate::app::AppError;
use std::collections::{HashMap, HashSet};

const REACHABLE: &str = "WITH RECURSIVE reachable(commit_id) AS (
    SELECT commit_id FROM commits WHERE oid = ?1
    UNION SELECT p.parent_id FROM commit_parents p JOIN reachable r USING(commit_id)
    WHERE p.parent_id IS NOT NULL
) ";

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
type Incarnation = (i64, Vec<u8>);
type Origins = HashMap<Incarnation, Option<Incarnation>>;

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
        let mut nodes = HashMap::new();
        let mut statement = self.connection.prepare(&format!(
            "{REACHABLE}SELECT commit_id, commit_time FROM commits JOIN reachable USING(commit_id)"
        )).map_err(cache_error)?;
        let rows = statement
            .query_map([target], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(cache_error)?;
        for row in rows {
            let (id, time) = row.map_err(cache_error)?;
            nodes.insert(
                id,
                Node {
                    time,
                    parents: vec![],
                    changes: vec![],
                },
            );
        }
        let mut statement = self.connection.prepare(&format!(
            "{REACHABLE}SELECT p.commit_id, p.parent_id FROM commit_parents p JOIN reachable USING(commit_id) ORDER BY p.commit_id, p.position"
        )).map_err(cache_error)?;
        let rows = statement
            .query_map([target], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
            })
            .map_err(cache_error)?;
        for row in rows {
            let (id, parent) = row.map_err(cache_error)?;
            nodes.get_mut(&id).unwrap().parents.push(parent);
        }
        let mut statement = self.connection.prepare(&format!(
            "{REACHABLE}SELECT c.commit_id, c.status, c.old_path, c.new_path FROM changes c JOIN reachable USING(commit_id) ORDER BY c.commit_id, c.ordinal"
        )).map_err(cache_error)?;
        let rows = statement
            .query_map([target], |row| {
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
        let mut renames: HashMap<Vec<u8>, HashSet<Vec<u8>>> = HashMap::new();
        for row in rows {
            let (id, change) = row.map_err(cache_error)?;
            if change.status.starts_with('R')
                && let (Some(old), Some(new)) = (&change.old, &change.new)
            {
                renames.entry(old.clone()).or_default().insert(new.clone());
                renames.entry(new.clone()).or_default().insert(old.clone());
            }
            nodes.get_mut(&id).unwrap().changes.push(change);
        }
        let target_id: i64 = self
            .connection
            .query_row(
                "SELECT commit_id FROM commits WHERE oid = ?1",
                [target],
                |row| row.get(0),
            )
            .map_err(cache_error)?;
        let mut origins = HashMap::new();
        Ok(paths
            .into_iter()
            .map(|path| {
                // Only detected-rename-connected paths can name this incarnation in another parent.
                let mut aliases = HashSet::new();
                let mut todo = vec![path.clone()];
                while let Some(alias) = todo.pop() {
                    if aliases.insert(alias.clone())
                        && let Some(neighbors) = renames.get(&alias)
                    {
                        todo.extend(neighbors.iter().cloned());
                    }
                }
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
                        .find(|c| c.new.as_deref() == Some(&alias));
                    if node.parents.len() <= 1 && change.is_some() {
                        touches.insert(id);
                    }
                    let previous = match change {
                        Some(c) if c.status.starts_with(['A', 'C']) => None,
                        Some(c) if c.status.starts_with('R') => c.old.as_ref(),
                        _ => Some(&alias),
                    };
                    let identity = if node.parents.len() > 1 {
                        origin(&nodes, id, &alias, &mut origins)
                    } else {
                        None
                    };
                    for (position, parent) in node.parents.iter().enumerate() {
                        let Some(parent) = parent else {
                            continue;
                        };
                        if position == 0 {
                            if let Some(path) = previous {
                                pending.push((*parent, path.clone()));
                            }
                        } else if let Some(identity) = &identity {
                            for candidate in &aliases {
                                if origin(&nodes, *parent, candidate, &mut origins).as_ref()
                                    == Some(identity)
                                {
                                    pending.push((*parent, candidate.clone()));
                                }
                            }
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

// Memoized first-parent origin: renames retain identity, A/C start it, D ends it.
// A merge introduction may already exist on another parent; merges never become touches.
fn origin(
    nodes: &HashMap<i64, Node>,
    mut id: i64,
    path: &[u8],
    memo: &mut Origins,
) -> Option<Incarnation> {
    let mut path = path.to_vec();
    let mut trail = vec![(id, path.clone())];
    let result = loop {
        let key = (id, path.clone());
        if let Some(result) = memo.get(&key) {
            break result.clone();
        }
        let Some(node) = nodes.get(&id) else {
            break None;
        };
        if let Some(change) = node
            .changes
            .iter()
            .find(|c| c.new.as_deref() == Some(&path))
        {
            // Cache query points and actual changes, not every untouched file/commit pair.
            trail.push(key.clone());
            if change.status.starts_with('A') {
                let inherited = node
                    .parents
                    .iter()
                    .skip(1)
                    .flatten()
                    .find_map(|parent| origin(nodes, *parent, &path, memo));
                break Some(inherited.unwrap_or(key));
            }
            if change.status.starts_with('C') {
                break Some(key);
            }
            if change.status.starts_with('R') {
                let Some(old) = &change.old else {
                    break None;
                };
                path = old.clone();
            }
        } else if node
            .changes
            .iter()
            .any(|c| c.old.as_deref() == Some(&path) && c.status.starts_with(['D', 'R']))
        {
            break None;
        }
        match node.parents.first().copied().flatten() {
            Some(parent) => id = parent,
            None => break None,
        }
    };
    for key in trail {
        memo.insert(key, result.clone());
    }
    result
}
