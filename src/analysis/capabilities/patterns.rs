//! Exact closed concrete-path combinations, independent of individual-path ranking.
use super::super::SearchScopeInfo;
use crate::{
    app::AppError,
    cache::{QuerySession, SearchFilter},
    git::Repository,
};
use std::collections::{BTreeSet, HashSet};

// Exact totals require visiting every closed combination; --limit cannot bound this.
// Fail explicitly rather than emitting partial counts on exponential histories.
const MAX_CLOSURES: usize = 4096;
const MAX_SUPPORT_CHECKS: usize = 1_000_000;
const BUDGET_ERROR: &str =
    "pattern discovery budget exceeded; narrow history scope or raise --min-support";
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
    pub(crate) citations: Vec<(String, i64)>,
    pub(crate) references_not_shown: usize,
}
pub(crate) struct Member {
    pub(crate) path: Vec<u8>,
    pub(crate) seed: bool,
    pub(crate) exists_at_target: bool,
}

pub(crate) fn run(
    session: &QuerySession,
    paths: &[String],
    scope: Option<&SearchFilter>,
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
    let target_revision = session.completed_tip()?;
    let existing = Repository::discover()?.file_paths_at(&target_revision)?;
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
        // Report scoped exclusions independently: an oversized merge meets both rules.
        let merge = observation.parent_count > 1;
        let mass = observation.paths.len() > 50;
        report.excluded_merges += usize::from(merge);
        report.excluded_mass_changes += usize::from(mass);
        if !merge && !mass && seeds.is_subset(&observation.paths) {
            observations.push(observation);
        }
    }
    report.eligible_seed_commits = observations.len();
    if observations.len() < min_support {
        return Ok(report);
    }
    let universe = observations
        .iter()
        .flat_map(|observation| observation.paths.iter().cloned())
        .collect::<BTreeSet<_>>();
    let closure = |supports: &[usize]| {
        let mut common = observations[supports[0]].paths.clone();
        for &index in &supports[1..] {
            common.retain(|path| observations[index].paths.contains(path));
        }
        common
    };
    let supports = (0..observations.len()).collect::<Vec<_>>();
    let initial = closure(&supports);
    let mut seen = HashSet::from([initial.clone()]);
    let mut pending = vec![(initial, supports)];
    let mut ranked = Vec::new();
    let mut support_checks = 0usize;
    // Traverse support-set closures, not every subset of each commit. Every frequent
    // closed combination is reachable by adding an item and closing its extent.
    while let Some((paths, supports)) = pending.pop() {
        for path in universe.difference(&paths) {
            support_checks = support_checks.saturating_add(supports.len());
            if support_checks > MAX_SUPPORT_CHECKS {
                return Err(AppError::input(BUDGET_ERROR));
            }
            let next = supports
                .iter()
                .copied()
                .filter(|&index| observations[index].paths.contains(path))
                .collect::<Vec<_>>();
            if next.len() < min_support {
                continue;
            }
            let common = closure(&next);
            if seen.insert(common.clone()) {
                if seen.len() > MAX_CLOSURES {
                    return Err(AppError::input(BUDGET_ERROR));
                }
                pending.push((common, next));
            }
        }
        if paths.len() < 3 || paths.len() == seeds.len() {
            continue;
        }
        let mut citations = supports
            .iter()
            .map(|&index| {
                (
                    observations[index].oid.clone(),
                    observations[index].commit_time,
                )
            })
            .collect::<Vec<_>>();
        citations.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        ranked.push((paths, citations));
    }
    ranked.sort_by(|left, right| {
        right
            .1
            .len()
            .cmp(&left.1.len())
            .then_with(|| right.1[0].1.cmp(&left.1[0].1))
            .then_with(|| left.0.cmp(&right.0))
    });
    report.matched_count = ranked.len();
    for (paths, mut citations) in ranked.into_iter().take(limit) {
        let support_count = citations.len();
        citations.truncate(5);
        report.patterns.push(Pattern {
            members: paths
                .into_iter()
                .map(|path| Member {
                    seed: seeds.contains(&path),
                    exists_at_target: existing.contains(&path),
                    path,
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
