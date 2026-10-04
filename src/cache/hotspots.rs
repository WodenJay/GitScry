//! Cache-only incarnation traversal; merge edges remain even when touches do not.
use super::QuerySession;
use crate::app::AppError;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

const REACHABLE: &str = "WITH RECURSIVE reachable(commit_id) AS (
    SELECT commit_id FROM commits WHERE ?1 IS NULL OR oid = ?1
    UNION SELECT p.parent_id FROM commit_parents p JOIN reachable r USING(commit_id)
    WHERE p.parent_id IS NOT NULL
) ";

const REACHABLE_ROOTS: &str = "WITH RECURSIVE reachable(commit_id) AS (
    SELECT commit_id FROM commits
    WHERE ?1 IS NULL AND EXISTS (
        SELECT 1 FROM temp.query_history_roots roots WHERE roots.oid = commits.oid
    )
    UNION SELECT p.parent_id FROM commit_parents p JOIN reachable r USING(commit_id)
    WHERE p.parent_id IS NOT NULL
) ";

fn cache_error(error: rusqlite::Error) -> AppError {
    super::cache_error("reading cached file history", error)
}
struct Change {
    id: i64,
    ordinal: i64,
    status: String,
    old: Option<Vec<u8>>,
    new: Option<Vec<u8>>,
    old_blob: Option<String>,
    new_blob: Option<String>,
    old_mode: String,
    new_mode: String,
}
struct Node {
    oid: String,
    time: i64,
    parents: Vec<Option<i64>>,
    changes: Vec<Change>,
}
type Incarnation = (i64, Vec<u8>);
type Origins = HashMap<Incarnation, Option<Incarnation>>;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct FileIncarnation {
    pub(crate) introduced_in: String,
    pub(crate) path: Vec<u8>,
}

#[derive(Default)]
pub(crate) struct FileIncarnationHistory {
    by_commit: HashMap<String, BTreeMap<FileIncarnation, BTreeSet<Vec<u8>>>>,
    changes_by_commit: HashMap<String, BTreeMap<i64, BTreeSet<FileIncarnation>>>,
    aliases: BTreeMap<Vec<u8>, BTreeSet<FileIncarnation>>,
    target_paths: BTreeMap<FileIncarnation, BTreeSet<Vec<u8>>>,
    target_incarnations: BTreeMap<String, BTreeMap<Vec<u8>, FileIncarnation>>,
}

impl FileIncarnationHistory {
    pub(crate) fn identities_for_paths(
        &self,
        paths: &BTreeSet<Vec<u8>>,
    ) -> BTreeSet<FileIncarnation> {
        paths
            .iter()
            .filter_map(|path| self.aliases.get(path))
            .flatten()
            .cloned()
            .collect()
    }

    /// A path may name multiple incarnations across history. Membership must
    /// match an identity, not merely a reused spelling of the path.
    pub(crate) fn contains_path(
        &self,
        path: &[u8],
        has_member: impl FnMut(&FileIncarnation) -> bool,
    ) -> bool {
        self.aliases
            .get(path)
            .is_some_and(|aliases| aliases.iter().any(has_member))
    }

    pub(crate) fn members_at(
        &self,
        revision: &str,
    ) -> Option<&BTreeMap<FileIncarnation, BTreeSet<Vec<u8>>>> {
        self.by_commit.get(revision)
    }

    pub(crate) fn identity_at(&self, revision: &str, path: &[u8]) -> Option<&FileIncarnation> {
        self.target_incarnations
            .get(revision)
            .and_then(|paths| paths.get(path))
    }

    pub(crate) fn change_has_identity(
        &self,
        revision: &str,
        ordinal: i64,
        identity: &FileIncarnation,
    ) -> bool {
        self.changes_by_commit
            .get(revision)
            .and_then(|changes| changes.get(&ordinal))
            .is_some_and(|identities| identities.contains(identity))
    }

    pub(crate) fn aliases_in(
        &self,
        identity: &FileIncarnation,
        paths: &BTreeSet<Vec<u8>>,
    ) -> Vec<Vec<u8>> {
        paths
            .iter()
            .filter(|path| {
                self.aliases
                    .get(*path)
                    .is_some_and(|identities| identities.contains(identity))
            })
            .cloned()
            .collect()
    }

    pub(crate) fn paths_at_target(&self, identity: &FileIncarnation) -> Vec<Vec<u8>> {
        self.target_paths
            .get(identity)
            .map(|paths| paths.iter().cloned().collect())
            .unwrap_or_default()
    }
}

struct HistoryGraph {
    nodes: HashMap<i64, Node>,
    renames: HashMap<Vec<u8>, HashSet<Vec<u8>>>,
}

fn load_history(
    connection: &rusqlite::Connection,
    target: Option<&str>,
    roots: Option<&[String]>,
) -> Result<HistoryGraph, AppError> {
    let reachable = if let Some(roots) = roots {
        connection
            .execute_batch(
                "CREATE TEMP TABLE IF NOT EXISTS query_history_roots (
                    oid TEXT NOT NULL PRIMARY KEY
                ) WITHOUT ROWID;
                DELETE FROM temp.query_history_roots;",
            )
            .map_err(cache_error)?;
        let mut statement = connection
            .prepare("INSERT OR IGNORE INTO temp.query_history_roots (oid) VALUES (?1)")
            .map_err(cache_error)?;
        for root in roots {
            statement.execute([root]).map_err(cache_error)?;
        }
        REACHABLE_ROOTS
    } else {
        REACHABLE
    };
    let mut nodes = HashMap::new();
    let mut statement = connection
        .prepare(&format!(
            "{reachable}SELECT commit_id, oid, commit_time FROM commits JOIN reachable USING(commit_id)"
        ))
        .map_err(cache_error)?;
    let rows = statement
        .query_map([target], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(cache_error)?;
    for row in rows {
        let (id, oid, time) = row.map_err(cache_error)?;
        nodes.insert(
            id,
            Node {
                oid,
                time,
                parents: vec![],
                changes: vec![],
            },
        );
    }

    let mut statement = connection
        .prepare(&format!(
            "{reachable}SELECT p.commit_id, p.parent_id FROM commit_parents p JOIN reachable USING(commit_id) ORDER BY p.commit_id, p.position"
        ))
        .map_err(cache_error)?;
    let rows = statement
        .query_map([target], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
        })
        .map_err(cache_error)?;
    for row in rows {
        let (id, parent) = row.map_err(cache_error)?;
        nodes.get_mut(&id).unwrap().parents.push(parent);
    }

    let mut statement = connection
        .prepare(&format!(
            "{reachable}SELECT c.commit_id, c.change_id, c.ordinal, c.status, c.old_path, c.new_path, c.old_blob, c.new_blob, c.old_mode, c.new_mode FROM changes c JOIN reachable USING(commit_id) ORDER BY c.commit_id, c.ordinal"
        ))
        .map_err(cache_error)?;
    let rows = statement
        .query_map([target], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                Change {
                    id: row.get(1)?,
                    ordinal: row.get(2)?,
                    status: row.get(3)?,
                    old: row.get(4)?,
                    new: row.get(5)?,
                    old_blob: row.get(6)?,
                    new_blob: row.get(7)?,
                    old_mode: row.get(8)?,
                    new_mode: row.get(9)?,
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
    Ok(HistoryGraph { nodes, renames })
}

#[derive(Default)]
struct TextChurn {
    additions: u64,
    deletions: u64,
}

// SHA-1 and SHA-256 object IDs for an empty Git blob.
const EMPTY_BLOB_OIDS: [&str; 2] = [
    "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391",
    "473a0f4c3be8a93681a267e3b1e9a7dcda1185436fe1419a30e160b89fdb5a34",
];

// Gitlink modes have no text blobs, so absent IDs never prove equality.
fn is_gitlink_change(change: &Change) -> bool {
    change.old_mode == "160000" || change.new_mode == "160000"
}

fn known_zero_churn(old_blob: Option<&str>, new_blob: Option<&str>) -> bool {
    old_blob.is_some() && old_blob == new_blob
        || (old_blob.is_none() && new_blob.is_some_and(|blob| EMPTY_BLOB_OIDS.contains(&blob)))
}

fn load_text_churn(
    connection: &rusqlite::Connection,
    change_ids: &HashSet<i64>,
) -> Result<HashMap<i64, TextChurn>, AppError> {
    let mut churn = HashMap::new();
    if change_ids.is_empty() {
        return Ok(churn);
    }

    let mut reader = super::HunkReader::new(connection)?;
    let change_ids = change_ids.iter().copied().collect::<Vec<_>>();
    for batch in change_ids.chunks(400) {
        let placeholders = vec!["?"; batch.len()].join(", ");
        let mut statement = connection
            .prepare(&format!(
                "SELECT change_id, payload_id FROM hunks WHERE change_id IN ({placeholders}) ORDER BY change_id, ordinal"
            ))
            .map_err(cache_error)?;
        let rows = statement
            .query_map(rusqlite::params_from_iter(batch.iter()), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(cache_error)?;
        for row in rows {
            let (change_id, payload_id) = row.map_err(cache_error)?;
            let text = reader.decode_payload(payload_id, &format!("change {change_id}"))?;
            for line in text.split_inclusive(|byte| *byte == b'\n') {
                let totals = churn.entry(change_id).or_default();
                let total = match line.first() {
                    Some(b'+') => &mut totals.additions,
                    Some(b'-') => &mut totals.deletions,
                    _ => continue,
                };
                *total = total
                    .checked_add(1)
                    .ok_or_else(|| AppError::operational("error: textual churn count overflow"))?;
            }
            reader.clear_decoded_blocks();
        }
    }
    Ok(churn)
}

fn add_lines(total: &mut u64, amount: u64) -> Result<(), AppError> {
    *total = total
        .checked_add(amount)
        .ok_or_else(|| AppError::operational("error: textual churn total overflow"))?;
    Ok(())
}

pub(crate) struct FileTouches {
    pub(crate) path: Vec<u8>,
    pub(crate) times: Vec<i64>,
    pub(crate) additions: Option<u64>,
    pub(crate) deletions: Option<u64>,
    pub(crate) churn_complete: bool,
}
impl QuerySession {
    pub(crate) fn pattern_incarnations(
        &self,
        target_revisions: &[String],
        current_paths: &[Vec<u8>],
    ) -> Result<FileIncarnationHistory, AppError> {
        let HistoryGraph { nodes, .. } = load_history(&self.connection, None, None)?;
        file_incarnation_history(&nodes, target_revisions, current_paths)
    }

    pub(crate) fn file_incarnations(
        &self,
        target_revisions: &[String],
        current_paths: &[Vec<u8>],
    ) -> Result<FileIncarnationHistory, AppError> {
        let HistoryGraph { nodes, .. } =
            load_history(&self.connection, None, Some(target_revisions))?;
        file_incarnation_history(&nodes, target_revisions, current_paths)
    }

    pub(crate) fn hotspot_touches(
        &self,
        target_revisions: &[String],
        paths: Vec<Vec<u8>>,
        eligible_revisions: Option<&HashSet<String>>,
    ) -> Result<Vec<FileTouches>, AppError> {
        let HistoryGraph { nodes, renames } =
            load_history(&self.connection, None, Some(target_revisions))?;
        let target_ids = target_revisions
            .iter()
            .map(|target| {
                nodes
                    .iter()
                    .find_map(|(&id, node)| (node.oid == *target).then_some(id))
                    .ok_or_else(|| cache_error(rusqlite::Error::QueryReturnedNoRows))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut origins = HashMap::new();
        let changes_by_id = nodes
            .values()
            .flat_map(|node| &node.changes)
            .map(|change| (change.id, change))
            .collect::<HashMap<_, _>>();
        let histories = paths
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
                let mut pending = target_ids
                    .iter()
                    .map(|id| (*id, path.clone()))
                    .collect::<Vec<_>>();
                let mut visited = HashSet::new();
                let mut touches = HashSet::new();
                let mut change_ids = HashSet::new();
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
                        .find(|change| change.new.as_deref() == Some(&alias));
                    if node.parents.len() <= 1
                        && let Some(change) = change
                        && eligible_revisions.is_none_or(|eligible| eligible.contains(&node.oid))
                    {
                        touches.insert(id);
                        change_ids.insert(change.id);
                    }
                    let previous = match change {
                        Some(change) if change.status.starts_with(['A', 'C']) => None,
                        Some(change) if change.status.starts_with('R') => change.old.as_ref(),
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
                (
                    FileTouches {
                        path,
                        times: touches.into_iter().map(|id| nodes[&id].time).collect(),
                        additions: None,
                        deletions: None,
                        churn_complete: false,
                    },
                    change_ids,
                )
            })
            .collect::<Vec<_>>();
        let change_ids = histories
            .iter()
            .flat_map(|(_, change_ids)| change_ids.iter().copied())
            .collect::<HashSet<_>>();
        let churn_change_ids = change_ids
            .iter()
            .copied()
            .filter(|change_id| {
                !changes_by_id
                    .get(change_id)
                    .is_some_and(|change| is_gitlink_change(change))
            })
            .collect::<HashSet<_>>();
        let churn = load_text_churn(&self.connection, &churn_change_ids)?;
        let mut files = Vec::with_capacity(histories.len());
        for (mut file, change_ids) in histories {
            let (mut additions, mut deletions, mut has_metrics, mut complete) = (0, 0, false, true);
            for change_id in change_ids {
                let Some(change) = changes_by_id.get(&change_id) else {
                    complete = false;
                    continue;
                };
                if is_gitlink_change(change) {
                    complete = false;
                } else if let Some(change_churn) = churn.get(&change_id) {
                    add_lines(&mut additions, change_churn.additions)?;
                    add_lines(&mut deletions, change_churn.deletions)?;
                    has_metrics = true;
                } else if known_zero_churn(change.old_blob.as_deref(), change.new_blob.as_deref()) {
                    has_metrics = true;
                } else {
                    complete = false;
                }
            }
            file.additions = has_metrics.then_some(additions);
            file.deletions = has_metrics.then_some(deletions);
            file.churn_complete = has_metrics && complete;
            files.push(file);
        }
        Ok(files)
    }
}

fn file_incarnation_history(
    nodes: &HashMap<i64, Node>,
    target_revisions: &[String],
    current_paths: &[Vec<u8>],
) -> Result<FileIncarnationHistory, AppError> {
    let mut history = FileIncarnationHistory::default();
    let mut origins = HashMap::new();

    for (&id, node) in nodes {
        let mut members: BTreeMap<FileIncarnation, BTreeSet<Vec<u8>>> = BTreeMap::new();
        let mut changes = BTreeMap::new();
        for change in &node.changes {
            let mut change_incarnations = BTreeSet::new();
            for (path, origin) in change_members(nodes, id, change, &mut origins) {
                let incarnation = file_incarnation(nodes, &origin);
                change_incarnations.insert(incarnation.clone());
                members
                    .entry(incarnation.clone())
                    .or_default()
                    .insert(path.clone());
                history.aliases.entry(path).or_default().insert(incarnation);
            }
            if !change_incarnations.is_empty() {
                changes.insert(change.ordinal, change_incarnations);
            }
        }
        if !members.is_empty() {
            history.by_commit.insert(node.oid.clone(), members);
        }
        if !changes.is_empty() {
            history.changes_by_commit.insert(node.oid.clone(), changes);
        }
    }

    for target in target_revisions {
        let target_id = nodes
            .iter()
            .find_map(|(&id, node)| (node.oid == *target).then_some(id))
            .ok_or_else(|| cache_error(rusqlite::Error::QueryReturnedNoRows))?;
        for path in current_paths {
            if let Some(origin) = origin(nodes, target_id, path, &mut origins) {
                let incarnation = file_incarnation(nodes, &origin);
                history
                    .target_paths
                    .entry(incarnation.clone())
                    .or_default()
                    .insert(path.clone());
                history
                    .target_incarnations
                    .entry(target.clone())
                    .or_default()
                    .insert(path.clone(), incarnation.clone());
                history
                    .aliases
                    .entry(path.clone())
                    .or_default()
                    .insert(incarnation);
            }
        }
    }
    Ok(history)
}

fn file_incarnation(nodes: &HashMap<i64, Node>, origin: &Incarnation) -> FileIncarnation {
    FileIncarnation {
        introduced_in: nodes[&origin.0].oid.clone(),
        path: origin.1.clone(),
    }
}

fn parent_origin(
    nodes: &HashMap<i64, Node>,
    id: i64,
    path: &[u8],
    memo: &mut Origins,
) -> Option<Incarnation> {
    nodes
        .get(&id)?
        .parents
        .first()
        .copied()
        .flatten()
        .and_then(|parent| origin(nodes, parent, path, memo))
}

fn change_members(
    nodes: &HashMap<i64, Node>,
    id: i64,
    change: &Change,
    memo: &mut Origins,
) -> Vec<(Vec<u8>, Incarnation)> {
    let mut members = Vec::new();
    if change.status.starts_with('R') {
        if let (Some(old), Some(new)) = (&change.old, &change.new)
            && let Some(origin) = origin(nodes, id, new, memo)
        {
            members.push((old.clone(), origin.clone()));
            members.push((new.clone(), origin));
        }
        return members;
    }

    if change.status.starts_with('C') {
        if let Some(old) = &change.old
            && Some(old) != change.new.as_ref()
            && let Some(origin) = parent_origin(nodes, id, old, memo)
        {
            members.push((old.clone(), origin));
        }
        if let Some(new) = &change.new
            && let Some(origin) = origin(nodes, id, new, memo)
        {
            members.push((new.clone(), origin));
        }
        return members;
    }

    if let Some(new) = &change.new
        && let Some(origin) = origin(nodes, id, new, memo)
    {
        members.push((new.clone(), origin));
    }
    if let Some(old) = &change.old
        && Some(old) != change.new.as_ref()
        && let Some(origin) = parent_origin(nodes, id, old, memo)
    {
        members.push((old.clone(), origin));
    }
    members
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

#[cfg(test)]
mod tests {
    use super::*;

    fn change(
        id: i64,
        ordinal: i64,
        status: &str,
        old: Option<&[u8]>,
        new: Option<&[u8]>,
    ) -> Change {
        Change {
            id,
            ordinal,
            status: status.to_owned(),
            old: old.map(<[u8]>::to_vec),
            new: new.map(<[u8]>::to_vec),
            old_blob: None,
            new_blob: None,
            old_mode: "100644".to_owned(),
            new_mode: "100644".to_owned(),
        }
    }

    #[test]
    fn records_each_change_under_its_own_file_incarnation() {
        let old_path = b"lib.rs";
        let new_path = b"score.rs";
        let nodes = HashMap::from([
            (
                1,
                Node {
                    oid: "base".to_owned(),
                    time: 0,
                    parents: Vec::new(),
                    changes: vec![change(1, 0, "A", None, Some(old_path))],
                },
            ),
            (
                2,
                Node {
                    oid: "head".to_owned(),
                    time: 1,
                    parents: vec![Some(1)],
                    changes: vec![
                        change(2, 0, "R100", Some(old_path), Some(new_path)),
                        change(3, 1, "A", None, Some(old_path)),
                    ],
                },
            ),
        ]);

        let history = file_incarnation_history(
            &nodes,
            &["head".to_owned()],
            &[old_path.to_vec(), new_path.to_vec()],
        )
        .unwrap();
        let original = history.identity_at("head", new_path).unwrap();
        let recreated = history.identity_at("head", old_path).unwrap();
        assert_ne!(original, recreated);
        assert_eq!(recreated.introduced_in, "head");
        assert!(history.change_has_identity("head", 0, original));
        assert!(!history.change_has_identity("head", 1, original));
        assert!(history.change_has_identity("head", 1, recreated));

        let seeds = BTreeSet::from([old_path.to_vec()]);
        assert_eq!(
            history.identities_for_paths(&seeds),
            BTreeSet::from([original.clone(), recreated.clone()])
        );
        assert!(history.contains_path(old_path, |identity| identity == original));
        assert!(!history.contains_path(new_path, |identity| identity == recreated));
        assert_eq!(
            history.aliases_in(original, &seeds),
            vec![old_path.to_vec()]
        );
        assert_eq!(history.paths_at_target(original), vec![new_path.to_vec()]);
        assert_eq!(history.paths_at_target(recreated), vec![old_path.to_vec()]);
        let members = history.members_at("head").unwrap();
        assert_eq!(
            members[original],
            BTreeSet::from([old_path.to_vec(), new_path.to_vec()])
        );
        assert_eq!(members[recreated], BTreeSet::from([old_path.to_vec()]));
    }
}
