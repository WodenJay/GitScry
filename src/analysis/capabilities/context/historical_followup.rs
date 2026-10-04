//! Qualify repeated content-origin to later-file-change chains within complete history windows.
use super::{Category, HistoricalFollowup, HistoricalFollowupChain, Suggestion};
use crate::{
    app::AppError,
    cache::{HistoryCommit, QuerySession, SearchFilter},
    git::{CurrentChange, Repository, current_regular_file},
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    path::Path,
};

const WINDOW_SECONDS: i64 = 7 * 24 * 60 * 60;
const MAX_PARENT_DISTANCE: usize = 20;
const BASELINE_SAMPLE_LIMIT: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Coverage {
    pub(crate) relationship_checks: usize,
    pub(crate) candidate_checks: usize,
    pub(crate) baseline_checks: usize,
    pub(crate) budget: Option<usize>,
    pub(crate) limited: bool,
}

impl Coverage {
    pub(crate) fn new(budget: Option<usize>) -> Self {
        Self {
            relationship_checks: 0,
            candidate_checks: 0,
            baseline_checks: 0,
            budget,
            limited: false,
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

struct Origin {
    oid: String,
    strength: usize,
    time: i64,
    associated_paths: BTreeSet<Vec<u8>>,
}

pub(super) fn discover(
    session: &QuerySession,
    repository: &Repository,
    root: &Path,
    input: &CurrentChange,
    content_origins: &[(usize, i64, Suggestion)],
    scope: Option<&SearchFilter>,
    coverage: &mut Coverage,
) -> Result<Vec<(usize, i64, Suggestion)>, AppError> {
    let Some(target) = scope
        .map(|scope| scope.to_oid.as_str())
        .or(input.head.as_deref())
    else {
        return Ok(Vec::new());
    };

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
    let active_revision = input.head.as_deref().unwrap_or(target);
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
    let eligible = reachable_cached
        .iter()
        .filter(|oid| {
            !excluded.contains(*oid)
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
    );
    let mut unique_origins = BTreeMap::<String, Origin>::new();
    for (strength, time, suggestion) in content_origins {
        let Some(citation) = suggestion.citations.first() else {
            continue;
        };
        let origin = unique_origins
            .entry(citation.oid.clone())
            .or_insert_with(|| Origin {
                oid: citation.oid.clone(),
                strength: *strength,
                time: *time,
                associated_paths: BTreeSet::new(),
            });
        origin.strength = origin.strength.max(*strength);
        origin.time = origin.time.max(*time);
        origin
            .associated_paths
            .extend(suggestion.associated_current_paths.iter().cloned());
    }
    let mut complete_content_origins = Vec::new();
    for origin in unique_origins.into_values() {
        if !eligible.contains(&origin.oid)
            || parents
                .get(&origin.oid)
                .is_none_or(|parents| parents.len() > 1)
        {
            continue;
        }
        let Some(&commit_time) = commit_times.get(&origin.oid) else {
            continue;
        };
        if observation_window_complete(commit_time, target_time, scope)
            && !incomplete_origins.contains(&origin.oid)
        {
            complete_content_origins.push(origin);
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
        if observation_window_complete(commit_time, target_time, scope)
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
    let selected_paths = input.paths().into_iter().collect::<HashSet<_>>();
    let mut candidate_paths = BTreeSet::new();
    for seed in candidate_seeds {
        for commit in session.timeline_history(&seed, &identity_cached)? {
            for path in commit.paths {
                if tracked_paths.contains(&path)
                    && !selected_paths.contains(&path)
                    && current_regular_file(root, &path)
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
        if complete_origin_count == 0 || supporting_origins * 2 < complete_origin_count {
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
            .map(|(origin_oid, later_oid)| HistoricalFollowupChain {
                origin_oid: origin_oid.clone(),
                later_oid: later_oid.clone(),
            })
            .collect::<Vec<_>>();
        let latest_support_time = chains
            .iter()
            .filter_map(|chain| commit_times.get(&chain.1).copied())
            .max()
            .unwrap_or_default();
        let mut associated_current_paths = associated_paths.into_iter().collect::<Vec<_>>();
        associated_current_paths.sort();
        let mut basis = vec![
            "same-file historical association through detected renames; not a causal conclusion"
                .to_owned(),
            "proper descendants within 7 days and 20 parent edges; merge diffs do not count as support"
                .to_owned(),
            format!(
                "supporting content origins: {supporting_origins}/{complete_origin_count}"
            ),
            format!(
                "candidate baseline: {baseline_occurrences}/{} sampled complete origins",
                baseline_sample_size
            ),
        ];
        if support_edges.len() > displayed_chains.len() {
            basis.push(format!(
                "{} additional supporting origins not shown as chains",
                support_edges.len() - displayed_chains.len()
            ));
        }
        results.push((
            supporting_origins,
            latest_support_time,
            Suggestion {
                category: Category::HistoricalFollowup,
                path,
                associated_current_paths,
                basis,
                selection_routes: vec!["historical_followup"],
                citations: Vec::new(),
                supporting_count: supporting_origins,
                citations_truncated: false,
                content_matches: Vec::new(),
                content_matches_truncated: false,
                abandonment: None,
                historical_followup: Some(HistoricalFollowup {
                    supporting_origins,
                    complete_origins: complete_origin_count,
                    independent_chains: chains.len(),
                    baseline_occurrences,
                    baseline_sample_size,
                    undisplayed_supporting_origins: support_edges
                        .len()
                        .saturating_sub(displayed_chains.len()),
                    chains: displayed_chains,
                }),
            },
        ));
    }
    Ok(results)
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
) -> bool {
    let Some(end) = origin_time.checked_add(WINDOW_SECONDS) else {
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
                let earliest = time.saturating_sub(WINDOW_SECONDS);
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
}

impl RelationshipGraph<'_> {
    fn later_descendants(
        &self,
        origin: &str,
        origin_time: i64,
        coverage: &mut Coverage,
        kind: RelationshipKind,
    ) -> Option<BTreeSet<String>> {
        let Some(end) = origin_time.checked_add(WINDOW_SECONDS) else {
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
