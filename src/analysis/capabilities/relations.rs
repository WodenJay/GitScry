use std::collections::{HashMap, HashSet};

use rusqlite::Connection;

use crate::app::AppError;

use super::super::retrieval;
use super::super::{Citation, Confidence, Detail, Intent, Material, Relation, Report, ReportKind};

const MASS_CHANGE_PATH_LIMIT: usize = 50;
const MAX_SUPPORTING_CITATIONS: usize = 5;

struct Support {
    oid: String,
    commit_time: i64,
    subject: String,
}

struct Candidate {
    path: Vec<u8>,
    key: String,
    total_touches: usize,
    supporting: Vec<Support>,
    seed_keys: HashSet<String>,
}

struct RankedCandidate {
    score: f64,
    latest_support_time: i64,
    key: String,
    material: Material,
}

pub(crate) fn related(
    connection: &Connection,
    intent: &Intent,
    worktree_paths: &[Vec<u8>],
    limit: usize,
) -> Result<Report, AppError> {
    run(connection, intent, worktree_paths, limit, false)
}

pub(crate) fn tests(
    connection: &Connection,
    intent: &Intent,
    worktree_paths: &[Vec<u8>],
    limit: usize,
) -> Result<Report, AppError> {
    run(connection, intent, worktree_paths, limit, true)
}

fn run(
    connection: &Connection,
    intent: &Intent,
    worktree_paths: &[Vec<u8>],
    limit: usize,
    tests_only: bool,
) -> Result<Report, AppError> {
    let seed_keys = intent.anchors().iter().cloned().collect::<HashSet<_>>();
    let current_keys = worktree_paths
        .iter()
        .map(|path| retrieval::normalize_path(path))
        .collect::<HashSet<_>>();
    let changes = retrieval::change_sets(connection)?;
    let (candidates, seed_touch_commits, eligible_commits, mass_changes_filtered) =
        collect_candidates(&changes, &seed_keys);
    let mut omitted_test_path = false;
    let mut ranked = Vec::new();

    for candidate in candidates.into_values() {
        let support_count = candidate.supporting.len();
        if support_count == 0 {
            continue;
        }
        let is_test = is_test_path(&candidate.path);
        if tests_only {
            if !is_test {
                continue;
            }
            if !current_keys.contains(&candidate.key) {
                omitted_test_path = true;
                continue;
            }
        }

        let proportion = support_count as f64 / seed_touch_commits as f64;
        let ubiquity = candidate.total_touches as f64 / eligible_commits as f64;
        let seed_coverage = candidate.seed_keys.len();
        let obvious_mirror = tests_only
            && intent
                .anchors()
                .iter()
                .any(|seed| obvious_mirror(seed, &candidate.path));
        let mut score = relation_score(
            support_count,
            proportion,
            ubiquity,
            seed_coverage,
            seed_keys.len(),
        );
        if tests_only && !obvious_mirror {
            score *= 1.25;
        }

        let mut supporting = candidate.supporting;
        supporting.sort_by(|left, right| {
            right
                .commit_time
                .cmp(&left.commit_time)
                .then_with(|| left.oid.cmp(&right.oid))
        });
        let latest_support_time = supporting
            .first()
            .map(|support| support.commit_time)
            .unwrap_or_default();
        let citations = supporting
            .iter()
            .take(MAX_SUPPORTING_CITATIONS)
            .map(|support| Citation::new(support.oid.clone(), support.subject.clone()))
            .collect::<Vec<_>>();
        let confidence = confidence(support_count, proportion);
        let mut basis = vec![
            format!("co-change count {support_count}"),
            format!(
                "candidate proportion of seed touches {:.1}%",
                proportion * 100.0
            ),
            format!("candidate ubiquity {:.1}%", ubiquity * 100.0),
            format!(
                "co-changed with {seed_coverage}/{} seed paths",
                seed_keys.len()
            ),
        ];
        if mass_changes_filtered {
            basis.push("mass-change commits excluded".to_owned());
        }
        if tests_only && !obvious_mirror {
            basis.push("historical support beyond mirrored test name".to_owned());
        }

        let material = Material {
            subject: String::new(),
            paths: vec![candidate.path],
            confidence,
            basis,
            citations,
            detail: Some(Detail::Relation(Relation {
                co_change_count: support_count,
                proportion,
                supporting_count: support_count,
            })),
        };
        ranked.push(RankedCandidate {
            score,
            latest_support_time,
            key: candidate.key,
            material,
        });
    }

    ranked.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| right.latest_support_time.cmp(&left.latest_support_time))
            .then_with(|| left.key.cmp(&right.key))
    });
    let matched_count = ranked.len();
    let mut materials = ranked
        .into_iter()
        .take(limit)
        .map(|candidate| candidate.material)
        .collect::<Vec<_>>();
    retrieval::assign_citations(&mut materials);

    let kind = if tests_only {
        ReportKind::Tests
    } else {
        ReportKind::Related
    };
    let mut report = super::super::report(kind, materials, matched_count, limit);
    if tests_only && omitted_test_path {
        report.warnings.push(
            "warning: unresolved test rename continuity may hide historical candidates.".to_owned(),
        );
    }
    Ok(report)
}

fn collect_candidates(
    changes: &[retrieval::ChangeSet],
    seed_keys: &HashSet<String>,
) -> (HashMap<String, Candidate>, usize, usize, bool) {
    let mut candidates = HashMap::new();
    let mut seed_touch_commits = 0;
    let mut eligible_commits = 0;
    let mut mass_changes_filtered = false;

    for change in changes {
        if change.is_merge {
            continue;
        }
        let paths = canonical_paths(change);
        if paths.len() <= 1 {
            continue;
        }
        if paths.len() > MASS_CHANGE_PATH_LIMIT {
            if paths.iter().any(|(key, _)| seed_keys.contains(key)) {
                mass_changes_filtered = true;
            }
            continue;
        }
        eligible_commits += 1;
        let touched_seeds = paths
            .iter()
            .filter(|(key, _)| seed_keys.contains(key))
            .map(|(key, _)| key.clone())
            .collect::<HashSet<_>>();
        for (key, path) in &paths {
            if seed_keys.contains(key) {
                continue;
            }
            let candidate = candidates.entry(key.clone()).or_insert_with(|| Candidate {
                path: path.clone(),
                key: key.clone(),
                total_touches: 0,
                supporting: Vec::new(),
                seed_keys: HashSet::new(),
            });
            candidate.total_touches += 1;
            if !touched_seeds.is_empty() {
                candidate.seed_keys.extend(touched_seeds.iter().cloned());
                candidate.supporting.push(Support {
                    oid: change.oid.clone(),
                    commit_time: change.commit_time,
                    subject: change.subject.clone(),
                });
            }
        }
        if !touched_seeds.is_empty() {
            seed_touch_commits += 1;
        }
    }

    (
        candidates,
        seed_touch_commits,
        eligible_commits,
        mass_changes_filtered,
    )
}

fn canonical_paths(change: &retrieval::ChangeSet) -> Vec<(String, Vec<u8>)> {
    let mut paths = Vec::new();
    for path in &change.paths {
        let key = retrieval::normalize_path(path);
        if !key.is_empty() && !paths.iter().any(|(known, _)| known == &key) {
            paths.push((key, path.clone()));
        }
    }
    paths
}

fn relation_score(
    support_count: usize,
    proportion: f64,
    ubiquity: f64,
    seed_coverage: usize,
    seed_count: usize,
) -> f64 {
    let breadth = if seed_count == 0 {
        0.0
    } else {
        seed_coverage as f64 / seed_count as f64
    };
    support_count as f64 * proportion * (1.0 + breadth * 0.5) / (1.0 + ubiquity)
}

fn confidence(support_count: usize, proportion: f64) -> Confidence {
    if support_count >= 4 && proportion >= 0.3 {
        Confidence::High
    } else if support_count >= 2 || proportion >= 0.1 {
        Confidence::Medium
    } else {
        Confidence::Low
    }
}

fn is_test_path(path: &[u8]) -> bool {
    let path = retrieval::normalize_path(path);
    let mut parts = path.rsplit('/');
    let name = parts.next().unwrap_or_default();
    if parts.any(|part| matches!(part, "test" | "tests" | "__tests__" | "spec")) {
        return true;
    }
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    stem.starts_with("test_")
        || stem.starts_with("test-")
        || stem.ends_with("_test")
        || stem.ends_with("_spec")
        || name.contains(".test.")
        || name.contains(".spec.")
}

fn obvious_mirror(seed: &str, candidate: &[u8]) -> bool {
    let seed = seed.rsplit('/').next().unwrap_or(seed);
    let seed = seed.rsplit_once('.').map_or(seed, |(stem, _)| stem);
    let candidate = retrieval::normalize_path(candidate);
    let candidate = candidate.rsplit('/').next().unwrap_or_default();
    let candidate = candidate
        .rsplit_once('.')
        .map_or(candidate, |(stem, _)| stem);
    let candidate = candidate
        .strip_prefix("test_")
        .or_else(|| candidate.strip_prefix("test-"))
        .or_else(|| candidate.strip_suffix("_test"))
        .unwrap_or(candidate);
    candidate == seed.to_ascii_lowercase()
}
