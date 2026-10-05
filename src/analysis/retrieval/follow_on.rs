//! Shared ancestry, observation-window and file-incarnation analysis.
use crate::{
    app::AppError,
    cache::{HistoryCommit, QuerySession, SearchFilter},
    git::Repository,
};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
const SECONDS_PER_DAY: i64 = 86400;
pub(crate) const MAX_PARENT_DISTANCE: usize = 20;
const BASELINE_SAMPLE_LIMIT: usize = 100;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Coverage {
    pub(crate) relationship_checks: usize,
    pub(crate) candidate_checks: usize,
    pub(crate) baseline_checks: usize,
    pub(crate) budget: Option<usize>,
    pub(crate) limited: bool,
    pub(crate) complete_origin_windows: usize,
    pub(crate) incomplete_origin_windows: usize,
}

impl Coverage {
    pub(crate) fn new(budget: Option<usize>) -> Self {
        Self {
            relationship_checks: 0,
            candidate_checks: 0,
            baseline_checks: 0,
            budget,
            limited: false,
            complete_origin_windows: 0,
            incomplete_origin_windows: 0,
        }
    }

    fn check(&mut self, kind: RelationshipKind) -> bool {
        if self
            .budget
            .is_some_and(|budget| self.relationship_checks >= budget)
        {
            self.limited = true;
            return false;
        }
        self.relationship_checks += 1;
        match kind {
            RelationshipKind::Candidate => self.candidate_checks += 1,
            RelationshipKind::Baseline => self.baseline_checks += 1,
        }
        true
    }
}

#[derive(Clone, Copy)]
enum RelationshipKind {
    Candidate,
    Baseline,
}
fn child_graph(parents: &HashMap<String, Vec<String>>) -> HashMap<String, Vec<String>> {
    let mut children = HashMap::<String, Vec<String>>::new();
    for (child, parent_oids) in parents {
        for parent in parent_oids {
            if parents.contains_key(parent) {
                children
                    .entry(parent.clone())
                    .or_default()
                    .push(child.clone());
            }
        }
    }
    for child_oids in children.values_mut() {
        child_oids.sort();
        child_oids.dedup();
    }
    children
}

fn stable_topological_order(parents: &HashMap<String, Vec<String>>) -> Vec<String> {
    let mut children = HashMap::<String, Vec<String>>::new();
    let mut remaining_parents = HashMap::new();
    for (oid, parent_oids) in parents {
        let in_graph = parent_oids
            .iter()
            .filter(|parent| parents.contains_key(*parent))
            .count();
        remaining_parents.insert(oid.clone(), in_graph);
        for parent in parent_oids {
            if parents.contains_key(parent) {
                children
                    .entry(parent.clone())
                    .or_default()
                    .push(oid.clone());
            }
        }
    }
    let mut ready = remaining_parents
        .iter()
        .filter_map(|(oid, count)| (*count == 0).then_some(oid.clone()))
        .collect::<BTreeSet<_>>();
    let mut order = Vec::with_capacity(parents.len());
    while let Some(oid) = ready.pop_first() {
        order.push(oid.clone());
        if let Some(child_oids) = children.get(&oid) {
            for child in child_oids {
                let count = remaining_parents
                    .get_mut(child)
                    .expect("child belongs to the graph");
                *count -= 1;
                if *count == 0 {
                    ready.insert(child.clone());
                }
            }
        }
    }
    order
}

fn observation_window_complete(
    origin_time: i64,
    target_time: i64,
    scope: Option<&SearchFilter>,
    window_seconds: i64,
) -> bool {
    let Some(end) = origin_time.checked_add(window_seconds) else {
        return false;
    };
    target_time >= end && scope.is_none_or(|scope| scope.until.is_none_or(|until| until >= end))
}

fn incomplete_windows(
    parents: &HashMap<String, Vec<String>>,
    commit_times: &HashMap<String, i64>,
    cached: &HashSet<String>,
    eligible: &HashSet<String>,
    excluded: &HashSet<String>,
    scope: Option<&SearchFilter>,
    window_seconds: i64,
) -> HashSet<String> {
    let mut incomplete = HashSet::new();
    for missing in parents.keys().filter(|oid| !cached.contains(*oid)) {
        if excluded.contains(missing) {
            continue;
        }
        let missing_time = commit_times.get(missing).copied();
        if missing_time.is_some_and(|time| {
            scope.is_some_and(|scope| {
                scope.since.is_some_and(|since| time < since)
                    || scope.until.is_some_and(|until| time > until)
            })
        }) {
            continue;
        }
        for (ancestor, distance) in ancestor_distances(missing, parents) {
            if distance == 0 || distance > MAX_PARENT_DISTANCE || !eligible.contains(&ancestor) {
                continue;
            }
            let Some(origin_time) = commit_times.get(&ancestor).copied() else {
                incomplete.insert(ancestor);
                continue;
            };
            let in_window = missing_time.is_none_or(|time| {
                let earliest = time.saturating_sub(window_seconds);
                origin_time >= earliest && origin_time <= time
            });
            if in_window {
                incomplete.insert(ancestor);
            }
        }
    }
    incomplete
}

fn ancestor_distances(
    descendant: &str,
    parents: &HashMap<String, Vec<String>>,
) -> HashMap<String, usize> {
    let mut distances = HashMap::from([(descendant.to_owned(), 0)]);
    let mut pending = VecDeque::from([(descendant.to_owned(), 0)]);
    while let Some((oid, distance)) = pending.pop_front() {
        if distance == MAX_PARENT_DISTANCE {
            continue;
        }
        if let Some(parent_oids) = parents.get(&oid) {
            for parent in parent_oids {
                if parents.contains_key(parent) && !distances.contains_key(parent) {
                    let parent_distance = distance + 1;
                    distances.insert(parent.clone(), parent_distance);
                    pending.push_back((parent.clone(), parent_distance));
                }
            }
        }
    }
    distances
}

fn within_window(
    oid: &str,
    origin_time: i64,
    end: i64,
    commit_times: &HashMap<String, i64>,
) -> bool {
    commit_times
        .get(oid)
        .is_some_and(|time| *time >= origin_time && *time <= end)
}
struct RelationshipGraph<'a> {
    children: &'a HashMap<String, Vec<String>>,
    commit_times: &'a HashMap<String, i64>,
    eligible: &'a HashSet<String>,
    parents: &'a HashMap<String, Vec<String>>,
    window_seconds: i64,
}

impl RelationshipGraph<'_> {
    fn later_descendants(
        &self,
        origin: &str,
        origin_time: i64,
        coverage: &mut Coverage,
        kind: RelationshipKind,
    ) -> Option<BTreeSet<String>> {
        let Some(end) = origin_time.checked_add(self.window_seconds) else {
            return Some(BTreeSet::new());
        };
        let mut seen = HashSet::from([origin.to_owned()]);
        let mut pending = VecDeque::from([(origin.to_owned(), 0usize)]);
        let mut later = BTreeSet::new();
        while let Some((oid, distance)) = pending.pop_front() {
            if distance == MAX_PARENT_DISTANCE {
                continue;
            }
            let Some(child_oids) = self.children.get(&oid) else {
                continue;
            };
            for child in child_oids {
                if !seen.insert(child.clone()) {
                    continue;
                }
                let child_distance = distance + 1;
                pending.push_back((child.clone(), child_distance));
                if self.eligible.contains(child)
                    && self
                        .parents
                        .get(child)
                        .is_some_and(|parents| parents.len() <= 1)
                    && within_window(child, origin_time, end, self.commit_times)
                {
                    if !coverage.check(kind) {
                        return None;
                    }
                    later.insert(child.clone());
                }
            }
        }
        Some(later)
    }
}

fn incarnation_changes(
    history: &[HistoryCommit],
    current_path: &[u8],
    target: &str,
    parents: &HashMap<String, Vec<String>>,
    cached: &HashSet<String>,
) -> HashSet<String> {
    let history_by_oid = history
        .iter()
        .enumerate()
        .map(|(index, commit)| (commit.oid.clone(), index))
        .collect::<HashMap<_, _>>();
    let mut changes = HashSet::new();
    let mut pending = vec![(target.to_owned(), current_path.to_vec())];
    let mut visited = HashSet::new();
    let mut path_existence = HashMap::new();

    while let Some((oid, path)) = pending.pop() {
        if !cached.contains(&oid) || !visited.insert((oid.clone(), path.clone())) {
            continue;
        }
        let change = path_change(history, &history_by_oid, &oid, &path);
        let commit_parents = parents.get(&oid).map(Vec::as_slice).unwrap_or(&[]);

        if commit_parents.len() > 1 {
            // A merge diff describes only its first parent. Trace every parent
            // containing this indexed path, but never count the merge diff.
            for parent in commit_parents {
                if path_exists_at_commit(
                    history,
                    &history_by_oid,
                    parent,
                    &path,
                    parents,
                    cached,
                    &mut path_existence,
                ) {
                    pending.push((parent.clone(), path.clone()));
                } else if change.is_some_and(|change| {
                    change.status.starts_with('R')
                        && change.new_path.as_deref() == Some(path.as_slice())
                }) && let Some(old_path) =
                    change.and_then(|change| change.old_path.as_ref())
                    && path_exists_at_commit(
                        history,
                        &history_by_oid,
                        parent,
                        old_path,
                        parents,
                        cached,
                        &mut path_existence,
                    )
                {
                    pending.push((parent.clone(), old_path.clone()));
                }
            }
            continue;
        }

        if let Some(change) = change {
            if change.new_path.as_deref() != Some(path.as_slice()) {
                continue;
            }
            changes.insert(oid.clone());
            if change.status.starts_with('A')
                || change.status.starts_with('C')
                || change.old_path.is_none()
            {
                continue;
            }
            let Some(parent) = commit_parents.first() else {
                continue;
            };
            let parent_path = if change.status.starts_with('R') {
                let Some(old_path) = &change.old_path else {
                    continue;
                };
                old_path.clone()
            } else {
                path
            };
            pending.push((parent.clone(), parent_path));
        } else if let Some(parent) = commit_parents.first() {
            pending.push((parent.clone(), path));
        }
    }
    changes
}

fn is_selected_source_incarnation(
    history: &[HistoryCommit],
    incarnation_changes: &HashSet<String>,
    selected_paths: &HashSet<Vec<u8>>,
) -> bool {
    history.iter().any(|commit| {
        incarnation_changes.contains(&commit.oid)
            && commit
                .changes
                .iter()
                .filter(|change| commit.anchored_ordinals.contains(&change.ordinal))
                .any(|change| {
                    change
                        .old_path
                        .as_deref()
                        .is_some_and(|path| selected_paths.contains(path))
                        || change
                            .new_path
                            .as_deref()
                            .is_some_and(|path| selected_paths.contains(path))
                })
    })
}

pub(crate) fn selected_source_incarnations(
    session: &QuerySession,
    repository: &Repository,
    revision: &str,
    selected_paths: &HashSet<Vec<u8>>,
    candidates: &[(Vec<u8>, String)],
) -> Result<HashSet<Vec<u8>>, AppError> {
    if selected_paths.is_empty() || candidates.is_empty() {
        return Ok(HashSet::new());
    }

    let parents = repository.reachable_commit_parents(revision)?;
    let cached_commits = session.commit_oids()?;
    let identity_cached = parents
        .keys()
        .filter(|oid| cached_commits.contains(*oid))
        .cloned()
        .collect::<HashSet<_>>();
    let mut histories = HashMap::<Vec<u8>, Vec<HistoryCommit>>::new();
    let mut excluded = HashSet::new();
    let mut checked_incarnations = HashMap::<Vec<u8>, HashSet<String>>::new();

    for (path, oid) in candidates {
        if excluded.contains(path) {
            continue;
        }
        if selected_paths.contains(path) {
            excluded.insert(path.clone());
            continue;
        }
        let checked = checked_incarnations.entry(path.clone()).or_default();
        if checked.contains(oid) {
            continue;
        }
        let history = match histories.entry(path.clone()) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(session.timeline_history(path, &identity_cached)?)
            }
        };
        let history_by_oid = history
            .iter()
            .enumerate()
            .map(|(index, commit)| (commit.oid.clone(), index))
            .collect::<HashMap<_, _>>();
        let Some(change) = path_change(history, &history_by_oid, oid, path) else {
            continue;
        };
        let target = if change.new_path.as_deref() == Some(path.as_slice()) {
            oid.as_str()
        } else if let Some(parent) = parents.get(oid).and_then(|parents| parents.first()) {
            parent.as_str()
        } else {
            continue;
        };
        let incarnation_changes =
            incarnation_changes(history, path, target, &parents, &identity_cached);
        checked.extend(incarnation_changes.iter().cloned());
        checked.insert(oid.clone());
        if is_selected_source_incarnation(history, &incarnation_changes, selected_paths) {
            excluded.insert(path.clone());
        }
    }

    Ok(excluded)
}

fn path_change<'a>(
    history: &'a [HistoryCommit],
    history_by_oid: &HashMap<String, usize>,
    oid: &str,
    path: &[u8],
) -> Option<&'a crate::cache::PathChange> {
    let commit = history.get(*history_by_oid.get(oid)?)?;
    commit.changes.iter().find(|change| {
        commit.anchored_ordinals.contains(&change.ordinal)
            && (change.old_path.as_deref() == Some(path)
                || change.new_path.as_deref() == Some(path))
    })
}

fn path_exists_at_commit(
    history: &[HistoryCommit],
    history_by_oid: &HashMap<String, usize>,
    oid: &str,
    path: &[u8],
    parents: &HashMap<String, Vec<String>>,
    cached: &HashSet<String>,
    cache: &mut HashMap<(String, Vec<u8>), bool>,
) -> bool {
    let mut current = oid.to_owned();
    let mut chain = Vec::new();
    let exists = loop {
        let key = (current.clone(), path.to_vec());
        if let Some(exists) = cache.get(&key) {
            break *exists;
        }
        if !cached.contains(&current) {
            break false;
        }
        chain.push(current.clone());
        if let Some(change) = path_change(history, history_by_oid, &current, path) {
            break change.new_path.as_deref() == Some(path);
        }
        let Some(first_parent) = parents.get(&current).and_then(|parents| parents.first()) else {
            break false;
        };
        current.clone_from(first_parent);
    };
    for commit in chain {
        cache.insert((commit, path.to_vec()), exists);
    }
    exists
}

fn uniform_sample_indices(length: usize, limit: usize) -> Vec<usize> {
    let count = length.min(limit);
    (0..count)
        .map(|index| (((2 * index + 1) as u128 * length as u128) / (2 * count as u128)) as usize)
        .collect()
}

fn independent_chains(edges: &BTreeMap<String, BTreeSet<String>>) -> Vec<(String, String)> {
    fn augment(
        origin_index: usize,
        origins: &[(&String, &BTreeSet<String>)],
        seen_later: &mut HashSet<String>,
        matched_later: &mut HashMap<String, usize>,
    ) -> bool {
        for later in origins[origin_index].1 {
            if !seen_later.insert(later.clone()) {
                continue;
            }
            let previous = matched_later.get(later).copied();
            if previous.is_none_or(|previous| augment(previous, origins, seen_later, matched_later))
            {
                matched_later.insert(later.clone(), origin_index);
                return true;
            }
        }
        false
    }

    let origins = edges.iter().collect::<Vec<_>>();
    let mut matched_later = HashMap::new();
    for origin_index in 0..origins.len() {
        augment(
            origin_index,
            &origins,
            &mut HashSet::new(),
            &mut matched_later,
        );
    }
    let mut chains = matched_later
        .into_iter()
        .map(|(later, origin_index)| (origins[origin_index].0.clone(), later))
        .collect::<Vec<_>>();
    chains.sort();
    chains
}

fn twice_baseline_is_met(
    supporting_origins: usize,
    complete_origins: usize,
    baseline_occurrences: usize,
    baseline_sample_size: usize,
) -> bool {
    (supporting_origins as u128) * (baseline_sample_size as u128)
        >= 2 * (baseline_occurrences as u128) * (complete_origins as u128)
}
pub(crate) struct Origin {
    pub(crate) oid: String,
    pub(crate) strength: usize,
    pub(crate) time: i64,
    pub(crate) associated_paths: BTreeSet<Vec<u8>>,
    pub(crate) path: Vec<u8>,
}

pub(crate) struct Origins<'a> {
    pub(crate) revision: &'a str,
    pub(crate) selected_paths: HashSet<Vec<u8>>,
    pub(crate) events: Vec<Origin>,
    pub(crate) require_current_file: bool,
}

pub(crate) struct Chain {
    pub(crate) origin_oid: String,
    pub(crate) later_oid: String,
    pub(crate) origin_path: Vec<u8>,
    pub(crate) origin_paths: Vec<Vec<u8>>,
    pub(crate) later_path: Vec<u8>,
    pub(crate) parent_distance: usize,
    pub(crate) elapsed_seconds: i64,
}

pub(crate) struct Observation {
    pub(crate) path: Vec<u8>,
    pub(crate) associated_current_paths: Vec<Vec<u8>>,
    pub(crate) supporting_origins: usize,
    pub(crate) eligible_origins: usize,
    pub(crate) independent_chains: usize,
    pub(crate) baseline_occurrences: usize,
    pub(crate) baseline_sample_size: usize,
    pub(crate) omitted_examples: usize,
    pub(crate) observation_days: usize,
    pub(crate) chains: Vec<Chain>,
    pub(crate) latest_support_time: i64,
}

pub(crate) struct PathObservation {
    pub(crate) source: Vec<u8>,
    pub(crate) observation: Observation,
}

pub(crate) struct PathAnalysis {
    pub(crate) observations: Vec<PathObservation>,
    pub(crate) complete_origin_windows: usize,
    pub(crate) incomplete_origin_windows: usize,
}
/// Each source is evaluated independently; no union of source events is an origin.
pub(crate) fn paths(
    session: &QuerySession,
    repository: &Repository,
    sources: &[Vec<u8>],
    selected_paths: &[Vec<u8>],
    scope: Option<&SearchFilter>,
) -> Result<PathAnalysis, AppError> {
    let revision = scope
        .ok_or_else(|| AppError::operational("missing pinned history scope"))?
        .to_oid
        .as_str();
    let parents = repository.reachable_commit_parents(revision)?;
    let cached = session.commit_oids()?;
    let reachable = parents
        .keys()
        .filter(|oid| cached.contains(*oid))
        .cloned()
        .collect();
    let mut results = PathAnalysis {
        observations: Vec::new(),
        complete_origin_windows: 0,
        incomplete_origin_windows: 0,
    };
    for source in sources {
        let history = session.timeline_history(source, &reachable)?;
        let changes = incarnation_changes(&history, source, revision, &parents, &cached);
        let events = history
            .iter()
            .filter(|commit| changes.contains(&commit.oid))
            .map(|commit| Origin {
                oid: commit.oid.clone(),
                strength: 1,
                time: commit.commit_time,
                associated_paths: BTreeSet::from([source.clone()]),
                path: commit
                    .changes
                    .iter()
                    .find(|change| commit.anchored_ordinals.contains(&change.ordinal))
                    .and_then(|change| change.new_path.clone())
                    .unwrap_or_else(|| source.clone()),
            })
            .collect();
        let mut coverage = Coverage::new(None);
        let observations = discover(
            session,
            repository,
            Origins {
                revision,
                selected_paths: selected_paths.iter().cloned().collect(),
                events,
                require_current_file: false,
            },
            7,
            scope,
            &mut coverage,
        )?;
        results.complete_origin_windows += coverage.complete_origin_windows;
        results.incomplete_origin_windows += coverage.incomplete_origin_windows;
        results
            .observations
            .extend(observations.into_iter().map(|observation| PathObservation {
                source: source.clone(),
                observation,
            }));
    }
    Ok(results)
}

pub(crate) fn discover(
    session: &QuerySession,
    repository: &Repository,
    origins: Origins<'_>,
    observation_days: usize,
    scope: Option<&SearchFilter>,
    coverage: &mut Coverage,
) -> Result<Vec<Observation>, AppError> {
    let window_seconds = i64::try_from(observation_days)
        .ok()
        .and_then(|days| days.checked_mul(SECONDS_PER_DAY))
        .filter(|seconds| *seconds > 0)
        .ok_or_else(|| {
            AppError::input("historical follow-up days must be a positive timestamp window")
        })?;
    let target = scope
        .map(|scope| scope.to_oid.as_str())
        .unwrap_or(origins.revision);

    let parents = repository.reachable_commit_parents(target)?;
    let commit_times = repository.reachable_commit_times(target)?;
    let Some(&target_time) = commit_times.get(target) else {
        return Ok(Vec::new());
    };
    let children = child_graph(&parents);
    let topo_order = stable_topological_order(&parents);
    let cached = session.commit_oids()?;
    let reachable_cached = parents
        .keys()
        .filter(|oid| cached.contains(*oid))
        .cloned()
        .collect::<HashSet<_>>();
    // Scope bounds evidence, but identity must follow the current HEAD file incarnation.
    let active_revision = origins.revision;
    let active_parents = if active_revision == target {
        None
    } else {
        Some(repository.reachable_commit_parents(active_revision)?)
    };
    let identity_parents = active_parents.as_ref().unwrap_or(&parents);
    let identity_cached = identity_parents
        .keys()
        .filter(|oid| cached.contains(*oid))
        .cloned()
        .collect::<HashSet<_>>();
    let excluded = match scope.and_then(|scope| scope.from_oid.as_deref()) {
        Some(from_oid) => repository
            .reachable_commits(from_oid)?
            .into_iter()
            .collect(),
        None => HashSet::new(),
    };
    // Boundary parent links are truncated, so their diffs cannot establish an
    // origin or fresh support event (a hidden merge could import existing work).
    // Keep boundaries in the traversal graph, but out of every event pool.
    let boundaries = repository
        .shallow_boundaries()?
        .into_iter()
        .collect::<HashSet<_>>();
    let eligible = reachable_cached
        .iter()
        .filter(|oid| {
            !excluded.contains(*oid)
                && !boundaries.contains(*oid)
                && scope.is_none_or(|scope| {
                    commit_times.get(*oid).is_some_and(|time| {
                        scope.since.is_none_or(|since| *time >= since)
                            && scope.until.is_none_or(|until| *time <= until)
                    })
                })
        })
        .cloned()
        .collect::<HashSet<_>>();

    let incomplete_origins = incomplete_windows(
        &parents,
        &commit_times,
        &cached,
        &eligible,
        &excluded,
        scope,
        window_seconds,
    );
    let unique_origins = origins.events;
    let mut complete_content_origins = Vec::new();
    for origin in unique_origins {
        if !eligible.contains(&origin.oid)
            || parents
                .get(&origin.oid)
                .is_none_or(|parents| parents.len() > 1)
        {
            continue;
        }
        let Some(&commit_time) = commit_times.get(&origin.oid) else {
            coverage.incomplete_origin_windows += 1;
            continue;
        };
        if observation_window_complete(commit_time, target_time, scope, window_seconds)
            && !incomplete_origins.contains(&origin.oid)
        {
            coverage.complete_origin_windows += 1;
            complete_content_origins.push(origin);
        } else {
            coverage.incomplete_origin_windows += 1;
        }
    }
    complete_content_origins.sort_by(|a, b| {
        b.strength
            .cmp(&a.strength)
            .then_with(|| b.time.cmp(&a.time))
            .then_with(|| a.oid.cmp(&b.oid))
    });
    if complete_content_origins.is_empty() {
        return Ok(Vec::new());
    }

    let mut complete_baseline_origins = Vec::new();
    for oid in &topo_order {
        if !eligible.contains(oid) || parents.get(oid).is_none_or(|parents| parents.len() > 1) {
            continue;
        }
        let Some(&commit_time) = commit_times.get(oid) else {
            continue;
        };
        if observation_window_complete(commit_time, target_time, scope, window_seconds)
            && !incomplete_origins.contains(oid)
        {
            complete_baseline_origins.push(oid.clone());
        }
    }

    let relationships = RelationshipGraph {
        children: &children,
        commit_times: &commit_times,
        eligible: &eligible,
        parents: &parents,
        window_seconds,
    };
    let mut relationship_windows = HashMap::<String, BTreeSet<String>>::new();
    let mut checked_content_origins = Vec::new();
    let mut candidate_seeds = BTreeSet::new();
    let mut scanned_descendants = HashSet::new();
    for origin in &complete_content_origins {
        let Some(descendants) = relationships.later_descendants(
            &origin.oid,
            commit_times[&origin.oid],
            coverage,
            RelationshipKind::Candidate,
        ) else {
            break;
        };
        for oid in &descendants {
            if scanned_descendants.insert(oid.clone()) {
                for change in session.changes(oid)? {
                    candidate_seeds.extend(change.old_path);
                    candidate_seeds.extend(change.new_path);
                }
            }
        }
        relationship_windows.insert(origin.oid.clone(), descendants);
        checked_content_origins.push(origin);
    }
    if candidate_seeds.is_empty() {
        return Ok(Vec::new());
    }

    let tracked_paths = repository
        .regular_files(active_revision)?
        .into_iter()
        .collect::<HashSet<_>>();
    let selected_paths = origins.selected_paths;
    let mut candidate_paths = BTreeSet::new();
    for seed in candidate_seeds {
        for commit in session.timeline_history(&seed, &identity_cached)? {
            for path in commit.paths {
                if tracked_paths.contains(&path)
                    && !selected_paths.contains(&path)
                    && (!origins.require_current_file
                        || crate::git::current_regular_file(&repository.root, &path))
                {
                    candidate_paths.insert(path);
                }
            }
        }
    }

    let mut results = Vec::new();

    for path in candidate_paths {
        let history = session.timeline_history(&path, &identity_cached)?;
        let incarnation_changes = incarnation_changes(
            &history,
            &path,
            active_revision,
            identity_parents,
            &identity_cached,
        );
        if incarnation_changes.is_empty() {
            continue;
        }
        if is_selected_source_incarnation(&history, &incarnation_changes, &selected_paths) {
            continue;
        }
        let scoped_changes = incarnation_changes
            .iter()
            .filter(|oid| {
                eligible.contains(*oid)
                    && parents.get(*oid).is_some_and(|parents| parents.len() <= 1)
            })
            .cloned()
            .collect::<BTreeSet<_>>();

        let mut support_edges = BTreeMap::<String, BTreeSet<String>>::new();
        let mut complete_origin_count = 0;
        let mut associated_paths = BTreeSet::new();
        for origin in &checked_content_origins {
            if incarnation_changes.contains(&origin.oid) {
                continue;
            }
            let later = relationship_windows
                .get(&origin.oid)
                .expect("complete content origin has a checked window")
                .intersection(&scoped_changes)
                .cloned()
                .collect::<BTreeSet<_>>();
            complete_origin_count += 1;
            if !later.is_empty() {
                support_edges.insert(origin.oid.clone(), later);
                associated_paths.extend(origin.associated_paths.iter().cloned());
            }
        }
        let supporting_origins = support_edges.len();
        if complete_origin_count == 0
            || supporting_origins as u128 * 2 < complete_origin_count as u128
        {
            continue;
        }
        let chains = independent_chains(&support_edges);
        if chains.len() < 2 {
            continue;
        }

        let baseline_pool = complete_baseline_origins
            .iter()
            .filter(|oid| !incarnation_changes.contains(*oid))
            .collect::<Vec<_>>();
        let sample_indices = uniform_sample_indices(baseline_pool.len(), BASELINE_SAMPLE_LIMIT);
        if sample_indices.is_empty() {
            continue;
        }
        let mut baseline_occurrences = 0;
        let mut baseline_sample_size = 0;
        for index in sample_indices.iter().copied() {
            let oid = baseline_pool[index];
            let Some(&commit_time) = commit_times.get(oid) else {
                continue;
            };
            let has_later = if let Some(later) = relationship_windows.get(oid) {
                later
                    .iter()
                    .any(|descendant| scoped_changes.contains(descendant))
            } else {
                let Some(later) = relationships.later_descendants(
                    oid,
                    commit_time,
                    coverage,
                    RelationshipKind::Baseline,
                ) else {
                    break;
                };
                let has_later = later
                    .iter()
                    .any(|descendant| scoped_changes.contains(descendant));
                relationship_windows.insert(oid.clone(), later);
                has_later
            };
            baseline_sample_size += 1;
            if has_later {
                baseline_occurrences += 1;
            }
        }
        if baseline_sample_size == 0 {
            continue;
        }
        if !twice_baseline_is_met(
            supporting_origins,
            complete_origin_count,
            baseline_occurrences,
            baseline_sample_size,
        ) {
            continue;
        }

        let displayed_chains = chains
            .iter()
            .take(2)
            .map(|(origin_oid, later_oid)| {
                let origin = checked_content_origins
                    .iter()
                    .find(|origin| &origin.oid == origin_oid)
                    .expect("support edge has a complete origin");
                Chain {
                    origin_oid: origin_oid.clone(),
                    later_oid: later_oid.clone(),
                    origin_path: origin.path.clone(),
                    origin_paths: origin.associated_paths.iter().cloned().collect(),
                    later_path: history
                        .iter()
                        .find(|commit| &commit.oid == later_oid)
                        .and_then(|commit| {
                            commit
                                .changes
                                .iter()
                                .find(|change| commit.anchored_ordinals.contains(&change.ordinal))
                        })
                        .and_then(|change| change.new_path.clone())
                        .unwrap_or_else(|| path.clone()),
                    parent_distance: ancestor_distances(later_oid, &parents)[origin_oid],
                    elapsed_seconds: commit_times[later_oid] - commit_times[origin_oid],
                }
            })
            .collect::<Vec<_>>();
        let latest_support_time = chains
            .iter()
            .filter_map(|chain| commit_times.get(&chain.1).copied())
            .max()
            .unwrap_or_default();
        let mut associated_current_paths = associated_paths.into_iter().collect::<Vec<_>>();
        associated_current_paths.sort();
        results.push(Observation {
            path,
            associated_current_paths,
            supporting_origins,
            eligible_origins: complete_origin_count,
            independent_chains: chains.len(),
            baseline_occurrences,
            baseline_sample_size,
            omitted_examples: chains.len().saturating_sub(displayed_chains.len()),
            observation_days,
            chains: displayed_chains,
            latest_support_time,
        });
    }
    Ok(results)
}
