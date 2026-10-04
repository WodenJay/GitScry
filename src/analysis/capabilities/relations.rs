use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use crate::{
    app::AppError,
    cache::{QuerySession, SearchFilter},
};

use super::super::retrieval;
use super::super::{Citation, Confidence, Detail, Intent, Material, Relation, Report, ReportKind};

const MASS_CHANGE_PATH_LIMIT: usize = 50;

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
    key: Vec<u8>,
    citation_oids: Vec<String>,
    material: Material,
}

pub(crate) fn related(
    session: &QuerySession,
    intent: &Intent,
    worktree_root: &Path,
    limit: usize,
    scope: Option<&SearchFilter>,
) -> Result<Report, AppError> {
    run(session, intent, worktree_root, limit, false, scope)
}

pub(crate) fn tests(
    session: &QuerySession,
    intent: &Intent,
    worktree_root: &Path,
    limit: usize,
    scope: Option<&SearchFilter>,
) -> Result<Report, AppError> {
    run(session, intent, worktree_root, limit, true, scope)
}

fn run(
    session: &QuerySession,
    intent: &Intent,
    worktree_root: &Path,
    limit: usize,
    tests_only: bool,
    scope: Option<&SearchFilter>,
) -> Result<Report, AppError> {
    let seed_keys = intent.anchors().iter().cloned().collect::<HashSet<_>>();
    let mut seed_list = seed_keys.iter().cloned().collect::<Vec<_>>();
    seed_list.sort();
    let retrieval::RelationHistory {
        candidates,
        seed_touch_commits,
        eligible_commits,
        mass_changes_filtered,
    } = session.relation_history(&seed_list, MASS_CHANGE_PATH_LIMIT, scope)?;
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

        let key = candidate.path.clone();
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
                follow_on: Vec::new(),
                co_change_citations: citation_oids.clone(),
            })),
            patch: None,
        };
        ranked.push(RankedCandidate {
            score,
            latest_support_time,
            key,
            citation_oids,
            material,
        });
    }

    if !tests_only {
        let repository = crate::git::Repository::discover()?;
        let sources = seed_list
            .iter()
            .map(|path| path.as_bytes().to_vec())
            .collect::<Vec<_>>();
        for evidence in retrieval::follow_on::paths(session, &repository, &sources, scope)? {
            let path = evidence.observation.path.clone();
            let latest = evidence.observation.latest_support_time;
            let score = evidence.observation.independent_chains as f64;
            let index = ranked.iter().position(|candidate| candidate.key == path);
            let candidate = if let Some(index) = index {
                &mut ranked[index]
            } else {
                ranked.push(RankedCandidate {
                    score,
                    latest_support_time: latest,
                    key: path.clone(),
                    citation_oids: Vec::new(),
                    material: Material {
                        subject: String::new(),
                        paths: vec![path],
                        confidence: Confidence::Medium,
                        basis: Vec::new(),
                        citations: Vec::new(),
                        patch: None,
                        detail: Some(Detail::Relation(Relation {
                            co_change_count: 0,
                            proportion: 0.0,
                            supporting_count: 0,
                            follow_on: Vec::new(),
                            co_change_citations: Vec::new(),
                        })),
                    },
                });
                ranked.last_mut().unwrap()
            };
            candidate.score = candidate.score.max(score);
            candidate.latest_support_time = candidate.latest_support_time.max(latest);
            for chain in &evidence.observation.chains {
                for oid in [&chain.origin_oid, &chain.later_oid] {
                    if !candidate.citation_oids.contains(oid) {
                        candidate.citation_oids.push(oid.clone());
                    }
                }
            }
            if let Some(Detail::Relation(relation)) = &mut candidate.material.detail {
                relation.follow_on.push(evidence);
            }
        }
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
                    let subject = retrieval::commit_text(session, oid)?
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

pub(super) fn relation_score(
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

pub(super) fn is_test_path(path: &[u8]) -> bool {
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
