//! Directory co-change uses historical path membership, not file lineage.
use std::collections::{BTreeSet, HashMap};

use super::{
    Citation, Detail, MASS_CHANGE_PATH_LIMIT, Material, ModuleCoChange, ModuleSourceSupport,
    ModuleSupport, Relation, RelationSource, Report, ReportKind, relation_score,
};
use crate::{app::AppError, cache::QuerySession};

pub(super) fn contains(directory: &str, path: &[u8]) -> bool {
    directory == "."
        || path
            .strip_prefix(directory.as_bytes())
            .is_some_and(|suffix| suffix.starts_with(b"/"))
}

fn source_contains(source: &RelationSource, path: &[u8], exact_file_path_exists: bool) -> bool {
    if source.kind == "directory" {
        return contains(&source.path, path);
    }
    if source.path.contains('/') {
        return if exact_file_path_exists {
            path == source.path.as_bytes()
        } else {
            ascii_case_eq(path, source.path.as_bytes())
        };
    }
    let basename = path.rsplit(|byte| *byte == b'/').next().unwrap_or(path);
    ascii_case_eq(basename, source.path.as_bytes())
}

fn ascii_case_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
}

#[derive(Clone)]
struct SourceCommit {
    oid: String,
    commit_time: i64,
    internal_paths: Vec<Vec<u8>>,
    sources: Vec<ModuleSourceSupport>,
}

pub(super) fn run(
    session: &QuerySession,
    observations: Vec<crate::cache::PatternObservation>,
    sources: Vec<RelationSource>,
    limit: usize,
) -> Result<Report, AppError> {
    let exact_file_paths = sources
        .iter()
        .map(|source| {
            source.kind == "file"
                && source.path.contains('/')
                && observations.iter().any(|commit| {
                    commit
                        .paths
                        .iter()
                        .any(|path| path.as_slice() == source.path.as_bytes())
                })
        })
        .collect::<Vec<_>>();
    let mut touches = 0;
    let mut eligible = 0;
    let mut mass_changes_filtered = false;
    let mut totals = HashMap::<Vec<u8>, usize>::new();
    let mut support = HashMap::<Vec<u8>, Vec<SourceCommit>>::new();
    for commit in observations {
        let source_support = sources
            .iter()
            .enumerate()
            .filter_map(|(index, source)| {
                let paths = commit
                    .paths
                    .iter()
                    .filter(|path| source_contains(source, path, exact_file_paths[index]))
                    .cloned()
                    .collect::<BTreeSet<_>>();
                (!paths.is_empty()).then(|| ModuleSourceSupport {
                    path: source.path.clone(),
                    kind: source.kind,
                    paths: paths.into_iter().collect(),
                })
            })
            .collect::<Vec<_>>();
        let internal = source_support
            .iter()
            .flat_map(|source| source.paths.iter().cloned())
            .collect::<BTreeSet<_>>();
        if commit.parent_count > 1 {
            continue;
        }
        if commit.paths.len() > MASS_CHANGE_PATH_LIMIT {
            mass_changes_filtered |= !internal.is_empty();
            continue;
        }
        // Retain the existing co-change eligibility: at least two distinct paths.
        if commit.paths.len() < 2 {
            continue;
        }
        eligible += 1;
        for path in &commit.paths {
            if !internal.contains(path) {
                *totals.entry(path.clone()).or_default() += 1;
            }
        }
        if source_support.is_empty() {
            continue;
        }
        touches += 1;
        let internal_paths = internal.iter().cloned().collect::<Vec<_>>();
        for path in commit.paths.iter().filter(|path| !internal.contains(*path)) {
            support.entry(path.clone()).or_default().push(SourceCommit {
                oid: commit.oid.clone(),
                commit_time: commit.commit_time,
                internal_paths: internal_paths.clone(),
                sources: source_support.clone(),
            });
        }
    }
    let mut ranked = Vec::new();
    for (path, mut commits) in support {
        commits.sort_by(|left, right| {
            right
                .commit_time
                .cmp(&left.commit_time)
                .then_with(|| left.oid.cmp(&right.oid))
        });
        let count = commits.len();
        let proportion = count as f64 / touches as f64;
        let ubiquity = totals[&path] as f64 / eligible as f64;
        let source_coverage = commits
            .iter()
            .flat_map(|commit| commit.sources.iter().map(|source| source.path.as_str()))
            .collect::<BTreeSet<_>>()
            .len();
        let score = relation_score(count, proportion, ubiquity, source_coverage, sources.len());
        let latest = commits[0].commit_time;
        ranked.push((score, latest, path, commits, proportion, ubiquity));
    }
    ranked.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    let matched_count = ranked.len();
    let mut materials = Vec::new();
    let mut subjects = HashMap::<String, String>::new();
    for (_, _, path, commits, proportion, ubiquity) in ranked.into_iter().take(limit) {
        let count = commits.len();
        let mut citations = Vec::new();
        let mut supporting = Vec::new();
        for commit in commits {
            let subject = if let Some(subject) = subjects.get(&commit.oid) {
                subject.clone()
            } else {
                let subject = super::retrieval::commit_text(session, &commit.oid)?
                    .map(|(subject, _)| subject)
                    .unwrap_or_default();
                subjects.insert(commit.oid.clone(), subject.clone());
                subject
            };
            citations.push(Citation::new(commit.oid.clone(), subject));
            supporting.push(ModuleSupport {
                oid: commit.oid,
                paths: commit.internal_paths,
                sources: commit.sources,
            });
        }
        let mut basis = vec![
            format!("co-change count {count}"),
            format!(
                "candidate proportion of source touches {:.1}%",
                proportion * 100.0
            ),
            format!("candidate ubiquity {:.1}%", ubiquity * 100.0),
        ];
        if mass_changes_filtered {
            basis.push("mass-change commits excluded".to_owned());
        }
        let citation_oids = citations
            .iter()
            .map(|citation| citation.oid.clone())
            .collect();
        materials.push(Material {
            subject: String::new(),
            paths: vec![path],
            confidence: super::confidence(count, proportion),
            basis,
            citations,
            detail: Some(Detail::Relation(Relation {
                co_change_count: count,
                proportion,
                supporting_count: count,
                follow_on: Vec::new(),
                co_change_citations: citation_oids,
                module: Some(ModuleCoChange {
                    touch_commits: touches,
                    support: supporting,
                }),
            })),
            patch: None,
        });
    }
    super::retrieval::assign_citations(&mut materials);
    let mut report =
        super::super::super::report(ReportKind::Related, materials, matched_count, limit);
    report.relation_sources = sources;
    Ok(report)
}
