//! Directory co-change uses historical path membership, not file lineage.
use std::collections::HashMap;

use super::{
    Citation, Detail, MASS_CHANGE_PATH_LIMIT, Material, ModuleCoChange, ModuleSupport, Relation,
    RelationSource, Report, ReportKind, relation_score,
};
use crate::{app::AppError, cache::QuerySession};

pub(super) fn contains(directory: &str, path: &[u8]) -> bool {
    path.strip_prefix(directory.as_bytes())
        .is_some_and(|suffix| suffix.starts_with(b"/"))
}

pub(super) fn run(
    session: &QuerySession,
    observations: Vec<crate::cache::PatternObservation>,
    sources: Vec<RelationSource>,
    limit: usize,
) -> Result<Report, AppError> {
    let directory = &sources[0].path;
    let mut touches = 0;
    let mut eligible = 0;
    let mut mass_changes_filtered = false;
    let mut totals = HashMap::<Vec<u8>, usize>::new();
    let mut support = HashMap::<Vec<u8>, Vec<(String, i64, Vec<Vec<u8>>)>>::new();
    for commit in observations {
        let internal = commit
            .paths
            .iter()
            .filter(|path| contains(directory, path))
            .cloned()
            .collect::<Vec<_>>();
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
            if !contains(directory, path) {
                *totals.entry(path.clone()).or_default() += 1;
            }
        }
        if internal.is_empty() {
            continue;
        }
        touches += 1;
        for path in commit
            .paths
            .iter()
            .filter(|path| !contains(directory, path))
        {
            support.entry(path.clone()).or_default().push((
                commit.oid.clone(),
                commit.commit_time,
                internal.clone(),
            ));
        }
    }
    let mut ranked = Vec::new();
    for (path, mut commits) in support {
        commits.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let count = commits.len();
        let proportion = count as f64 / touches as f64;
        let ubiquity = totals[&path] as f64 / eligible as f64;
        let score = relation_score(count, proportion, ubiquity, 1, 1);
        let latest = commits[0].1;
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
        for (oid, _, paths) in commits {
            let subject = if let Some(subject) = subjects.get(&oid) {
                subject.clone()
            } else {
                let subject = super::retrieval::commit_text(session, &oid)?
                    .map(|(subject, _)| subject)
                    .unwrap_or_default();
                subjects.insert(oid.clone(), subject.clone());
                subject
            };
            citations.push(Citation::new(oid.clone(), subject));
            supporting.push(ModuleSupport { oid, paths });
        }
        let mut basis = vec![
            format!("co-change count {count}"),
            format!(
                "candidate proportion of module touches {:.1}%",
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
