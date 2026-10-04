//! Closed file-incarnation combinations, independent of individual-path ranking.
use super::super::SearchScopeInfo;
use crate::analysis::Intent;
use crate::analysis::query::{Context, Options, Outcome, QueryReport, scope};
use crate::{
    app::AppError,
    cache::{FileIncarnation, FileIncarnationHistory, QuerySession, SearchFilter},
    git::Repository,
};
use std::collections::{BTreeMap, BTreeSet, HashSet};

pub(in crate::analysis) fn execute(
    paths: Vec<String>,
    min_support: usize,
    options: Options,
) -> Result<Outcome, AppError> {
    Intent::paths(&paths)?;
    let context = Context::open(options.scope)?;
    let repository = Repository::discover()?;
    let target_revision = context.pinned_head.clone();
    let cached_targets =
        scope::cached_head_history_frontier(&context.session, &repository, &context.pinned_head)?;
    let report = run(
        &context.session,
        &paths,
        context.filter(),
        &target_revision,
        &cached_targets,
        min_support,
        options.limit,
    )?;
    Ok(context.finish(QueryReport::Patterns(report)))
}

pub(crate) struct Report {
    pub(crate) target_revision: String,
    pub(crate) scope: Option<SearchScopeInfo>,
    pub(crate) eligible_seed_commits: usize,
    pub(crate) excluded_merges: usize,
    pub(crate) excluded_mass_changes: usize,
    pub(crate) matched_count: usize,
    pub(crate) patterns: Vec<Pattern>,
}
pub(crate) struct Pattern {
    pub(crate) members: Vec<Member>,
    pub(crate) support_count: usize,
    pub(crate) proportion: f64,
    pub(crate) citations: Vec<Citation>,
    pub(crate) references_not_shown: usize,
}
pub(crate) struct Member {
    pub(crate) path: Vec<u8>,
    pub(crate) introduced_in: String,
    pub(crate) seed: bool,
    pub(crate) exists_at_target: bool,
    pub(crate) target_paths: Vec<Vec<u8>>,
}
pub(crate) struct Citation {
    pub(crate) oid: String,
    pub(crate) commit_time: i64,
    pub(crate) members: Vec<CitationMember>,
}
pub(crate) struct CitationMember {
    pub(crate) introduced_in: String,
    pub(crate) introduced_path: Vec<u8>,
    pub(crate) changed_paths: Vec<Vec<u8>>,
}
struct Observation {
    oid: String,
    commit_time: i64,
    members: BTreeMap<FileIncarnation, BTreeSet<Vec<u8>>>,
}

fn run(
    session: &QuerySession,
    paths: &[String],
    scope: Option<&SearchFilter>,
    target_revision: &str,
    cached_targets: &[String],
    min_support: usize,
    limit: usize,
) -> Result<Report, AppError> {
    if min_support < 2 {
        return Err(AppError::input("minimum support must be at least two"));
    }
    let seeds = paths
        .iter()
        .map(|path| {
            let mut path = path.replace('\\', "/");
            while let Some(stripped) = path.strip_prefix("./") {
                path = stripped.to_owned();
            }
            path.trim_end_matches('/').as_bytes().to_vec()
        })
        .collect::<BTreeSet<_>>();
    let target_revision = target_revision.to_owned();
    let current_paths = Repository::discover()?.file_paths_at(&target_revision)?;
    let current_paths = current_paths.into_iter().collect::<Vec<_>>();
    let incarnations: FileIncarnationHistory =
        session.pattern_incarnations(cached_targets, &current_paths)?;
    let seed_incarnations = seeds
        .iter()
        .filter_map(|seed| incarnations.aliases.get(seed))
        .flatten()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut report = Report {
        target_revision,
        scope: None,
        eligible_seed_commits: 0,
        excluded_merges: 0,
        excluded_mass_changes: 0,
        matched_count: 0,
        patterns: Vec::new(),
    };
    let mut observations = Vec::new();
    for observation in session.pattern_observations(scope)? {
        let merge = observation.parent_count > 1;
        let mass = observation.paths.len() > 50;
        report.excluded_merges += usize::from(merge);
        report.excluded_mass_changes += usize::from(mass);
        if merge || mass {
            continue;
        }
        let Some(members) = incarnations.by_commit.get(&observation.oid) else {
            continue;
        };
        let all_seeds_present = seeds.iter().all(|seed| {
            incarnations.aliases.get(seed).is_some_and(|aliases| {
                aliases
                    .iter()
                    .any(|identity| members.contains_key(identity))
            })
        });
        if all_seeds_present {
            observations.push(Observation {
                oid: observation.oid,
                commit_time: observation.commit_time,
                members: members.clone(),
            });
        }
    }
    report.eligible_seed_commits = observations.len();
    if observations.len() < min_support {
        return Ok(report);
    }
    let universe = observations
        .iter()
        .flat_map(|observation| observation.members.keys().cloned())
        .collect::<BTreeSet<_>>();
    let closure = |supports: &[usize]| {
        let mut common = observations[supports[0]]
            .members
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        for &index in &supports[1..] {
            common.retain(|identity| observations[index].members.contains_key(identity));
        }
        common
    };
    let mut seen = HashSet::new();
    let mut pending = Vec::new();
    for seed in &seed_incarnations {
        let supports = observations
            .iter()
            .enumerate()
            .filter_map(|(index, observation)| {
                observation.members.contains_key(seed).then_some(index)
            })
            .collect::<Vec<_>>();
        if supports.len() < min_support {
            continue;
        }
        let initial = closure(&supports);
        if seen.insert(initial.clone()) {
            pending.push((initial, supports));
        }
    }
    let mut ranked = Vec::new();
    // Traverse support-set closures, not every subset of each commit. Every frequent
    // closed combination is reachable by adding an item and closing its extent.
    while let Some((members, supports)) = pending.pop() {
        for identity in universe.difference(&members) {
            let next = supports
                .iter()
                .copied()
                .filter(|&index| observations[index].members.contains_key(identity))
                .collect::<Vec<_>>();
            if next.len() < min_support {
                continue;
            }
            let common = closure(&next);
            if seen.insert(common.clone()) {
                pending.push((common, next));
            }
        }
        let all_seeds_present = seeds.iter().all(|seed| {
            incarnations
                .aliases
                .get(seed)
                .is_some_and(|aliases| aliases.iter().any(|identity| members.contains(identity)))
        });
        let has_non_seed = members
            .iter()
            .any(|identity| !seed_incarnations.contains(identity));
        if members.len() < 3 || !all_seeds_present || !has_non_seed {
            continue;
        }
        let mut citations = supports
            .iter()
            .map(|&index| Citation {
                oid: observations[index].oid.clone(),
                commit_time: observations[index].commit_time,
                members: members
                    .iter()
                    .map(|identity| CitationMember {
                        introduced_in: identity.introduced_in.clone(),
                        introduced_path: identity.path.clone(),
                        changed_paths: observations[index].members[identity]
                            .iter()
                            .cloned()
                            .collect(),
                    })
                    .collect(),
            })
            .collect::<Vec<_>>();
        citations.sort_by(|left, right| {
            right
                .commit_time
                .cmp(&left.commit_time)
                .then_with(|| left.oid.cmp(&right.oid))
        });
        ranked.push((members, citations));
    }
    ranked.sort_by(|left, right| {
        right
            .1
            .len()
            .cmp(&left.1.len())
            .then_with(|| right.1[0].commit_time.cmp(&left.1[0].commit_time))
            .then_with(|| left.0.cmp(&right.0))
    });
    report.matched_count = ranked.len();
    for (members, mut citations) in ranked.into_iter().take(limit) {
        let support_count = citations.len();
        citations.truncate(5);
        report.patterns.push(Pattern {
            members: members
                .into_iter()
                .map(|identity| {
                    let target_paths = incarnations
                        .target_paths
                        .get(&identity)
                        .map(|paths| paths.iter().cloned().collect::<Vec<_>>())
                        .unwrap_or_default();
                    Member {
                        path: identity.path.clone(),
                        introduced_in: identity.introduced_in.clone(),
                        seed: seed_incarnations.contains(&identity),
                        exists_at_target: !target_paths.is_empty(),
                        target_paths,
                    }
                })
                .collect(),
            support_count,
            proportion: support_count as f64 / observations.len() as f64,
            references_not_shown: support_count - citations.len(),
            citations,
        });
    }
    Ok(report)
}
