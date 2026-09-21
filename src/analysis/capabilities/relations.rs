use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use rusqlite::Connection;

use crate::app::AppError;

use super::super::retrieval;
use super::super::{Citation, Confidence, Detail, Intent, Material, Relation, Report, ReportKind};

const MASS_CHANGE_PATH_LIMIT: usize = 50;
const MAX_SUPPORTING_CITATIONS: usize = 5;

#[cfg(unix)]
fn current_path_is_file(root: &Path, path: &[u8]) -> bool {
    use std::os::unix::ffi::OsStrExt;

    root.join(std::ffi::OsStr::from_bytes(path)).is_file()
}

#[cfg(not(unix))]
fn current_path_is_file(root: &Path, path: &[u8]) -> bool {
    root.join(String::from_utf8_lossy(path).as_ref()).is_file()
}
struct RankedCandidate {
    score: f64,
    latest_support_time: i64,
    key: String,
    citation_oids: Vec<String>,
    material: Material,
}

pub(crate) fn related(
    connection: &Connection,
    intent: &Intent,
    worktree_root: &Path,
    limit: usize,
) -> Result<Report, AppError> {
    run(connection, intent, worktree_root, limit, false)
}

pub(crate) fn tests(
    connection: &Connection,
    intent: &Intent,
    worktree_root: &Path,
    limit: usize,
) -> Result<Report, AppError> {
    run(connection, intent, worktree_root, limit, true)
}

fn run(
    connection: &Connection,
    intent: &Intent,
    worktree_root: &Path,
    limit: usize,
    tests_only: bool,
) -> Result<Report, AppError> {
    let seed_keys = intent.anchors().iter().cloned().collect::<HashSet<_>>();
    let seed_list = seed_keys.iter().cloned().collect::<Vec<_>>();
    let retrieval::RelationHistory {
        candidates,
        seed_touch_commits,
        eligible_commits,
        mass_changes_filtered,
    } = retrieval::relation_history(connection, &seed_list, MASS_CHANGE_PATH_LIMIT)?;
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
            if !current_path_is_file(worktree_root, &candidate.path) {
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
        let citation_oids = supporting
            .iter()
            .take(MAX_SUPPORTING_CITATIONS)
            .map(|support| support.oid.clone())
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

        let key = candidate.key;
        let material = Material {
            subject: String::new(),
            paths: vec![candidate.path],
            confidence,
            basis,
            citations: Vec::new(),
            detail: Some(Detail::Relation(Relation {
                co_change_count: support_count,
                proportion,
                supporting_count: support_count,
            })),
        };
        ranked.push(RankedCandidate {
            score,
            latest_support_time,
            key,
            citation_oids,
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
    let mut citation_subjects = HashMap::<String, String>::new();
    let mut materials = Vec::new();
    for candidate in ranked.into_iter().take(limit) {
        let citations = candidate
            .citation_oids
            .iter()
            .map(|oid| {
                let subject = if let Some(subject) = citation_subjects.get(oid) {
                    subject.clone()
                } else {
                    let subject = retrieval::commit_text(connection, oid)?
                        .map(|(subject, _)| subject)
                        .unwrap_or_default();
                    citation_subjects.insert(oid.clone(), subject.clone());
                    subject
                };
                Ok(Citation::new(oid.clone(), subject))
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        let mut material = candidate.material;
        material.citations = citations;
        materials.push(material);
    }
    retrieval::assign_citations(&mut materials);

    let kind = if tests_only {
        ReportKind::Tests
    } else {
        ReportKind::Related
    };
    let mut report = super::super::report(kind, materials, matched_count, limit);
    if tests_only && omitted_test_path {
        report.warnings.push(
            "warning: historical test paths absent from the current worktree may include unresolved renames."
                .to_owned(),
        );
    }
    Ok(report)
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
