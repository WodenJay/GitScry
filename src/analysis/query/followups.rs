//! Bounded same-file material. Identity is branch-local and never resumes after ending.
use std::collections::{BTreeMap, HashMap, HashSet};

use super::{Options, Outcome, QueryReport};
use crate::{
    app::AppError,
    cache::{self, followups::ForwardCommit},
    git::Repository,
};
/// Cache positions are the published reverse-topological Git order, not timestamps.
/// Mark descendants using all parents, excluding parallel endpoint-reachable work.
fn descendants(graph: &[ForwardCommit], seed: &str) -> HashSet<String> {
    let mut descendants = HashSet::from([seed.to_owned()]);
    for node in graph {
        if node
            .parents
            .iter()
            .any(|parent| descendants.contains(parent))
        {
            descendants.insert(node.oid.clone());
        }
    }
    descendants.remove(seed);
    descendants
}

type Incarnations = BTreeMap<Vec<u8>, bool>;

pub(crate) struct Report {
    pub(crate) scope: Scope,
    pub(crate) inspected_count: usize,
    pub(crate) lineage_inspected_count: usize,
    pub(crate) inspected_first: Option<String>,
    pub(crate) inspected_last: Option<String>,
    pub(crate) traversal_truncated: bool,
    pub(crate) display_truncated: bool,
    pub(crate) matched_in_inspected_scope: usize,
    pub(crate) entries: Vec<Entry>,
    pub(crate) warnings: Vec<String>,
}

pub(crate) struct Scope {
    pub(crate) seed: String,
    pub(crate) endpoint: String,
    pub(crate) cache_tip: String,
    pub(crate) seed_time: i64,
    pub(crate) time_ceiling: i64,
    pub(crate) days: usize,
    pub(crate) max_commits: usize,
    pub(crate) limit: usize,
    pub(crate) selected_paths: Vec<Vec<u8>>,
}

pub(crate) struct Entry {
    pub(crate) commit_id: String,
    pub(crate) subject: String,
    pub(crate) commit_time: i64,
    pub(crate) elapsed_seconds: i64,
    pub(crate) paths: Vec<Vec<u8>>,
    pub(crate) change_types: Vec<String>,
    pub(crate) revert_reference: Option<String>,
    pub(crate) same_file_association: bool,
    pub(crate) parent_count: usize,
    pub(crate) patch: Option<crate::analysis::PatchExcerpt>,
}

pub(super) fn run(
    revision: String,
    paths: Vec<String>,
    to_rev: Option<String>,
    days: usize,
    max_commits: usize,
    options: Options,
) -> Result<Outcome, AppError> {
    let seconds = i64::try_from(days)
        .ok()
        .and_then(|days| days.checked_mul(86400))
        .filter(|_| days > 0 && max_commits > 0)
        .ok_or_else(|| AppError::input("followups bounds must be positive and representable"))?;
    for path in &paths {
        validate_path(path)?;
    }
    let repository = Repository::discover()?;
    let session = cache::open_query(&repository.root)?;
    let seed = repository.resolve_commit(&revision)?;
    session.require_revision(&seed)?;
    let cache_tip = session.completed_tip()?;
    let endpoint = match to_rev {
        Some(revision) => repository.resolve_commit(&revision)?,
        None => cache_tip.clone(),
    };
    session.require_revision(&endpoint)?;
    if !session.ancestors(&endpoint)?.contains(&seed) {
        return Err(AppError::input(
            "followups seed must be an ancestor of endpoint",
        ));
    }
    let graph = session.forward_graph(&endpoint)?;
    let seed_time = graph
        .iter()
        .find(|node| node.oid == seed)
        .expect("cached reachable seed")
        .commit_time;
    let time_ceiling = seed_time
        .checked_add(seconds)
        .ok_or_else(|| AppError::input("--days produces an unrepresentable time ceiling"))?;
    // Also guarantee all signed elapsed values are representable before inspection.
    let descendants = descendants(&graph, &seed);
    if graph.iter().any(|node| {
        descendants.contains(&node.oid) && node.commit_time.checked_sub(seed_time).is_none()
    }) {
        return Err(AppError::input(
            "descendant elapsed time is unrepresentable",
        ));
    }
    let original = session.forward_changes(&seed)?;
    for path in &paths {
        if !original.iter().any(|change| {
            [&change.old_path, &change.new_path]
                .into_iter()
                .flatten()
                .any(|p| p == path.as_bytes())
        }) {
            return Err(AppError::input(format!(
                "path was not changed by seed revision: {path}"
            )));
        }
    }
    let mut initial = Incarnations::new();
    let mut selected_paths = Vec::new();
    for change in &original {
        if !paths.is_empty()
            && ![&change.old_path, &change.new_path]
                .into_iter()
                .flatten()
                .any(|p| paths.iter().any(|path| p == path.as_bytes()))
        {
            continue;
        }
        if let Some(path) = change.new_path.as_ref().or(change.old_path.as_ref()) {
            selected_paths.push(path.clone());
            initial.insert(
                path.clone(),
                !change.status.starts_with('D') && change.new_path.is_some(),
            );
        }
    }
    selected_paths.sort();
    selected_paths.dedup();
    let mut report = Report {
        scope: Scope { seed: seed.clone(), endpoint, cache_tip, seed_time, time_ceiling, days, max_commits, limit: options.limit, selected_paths },
        inspected_count: 0, lineage_inspected_count: 0, inspected_first: None, inspected_last: None,
        traversal_truncated: false, display_truncated: false, matched_in_inspected_scope: 0,
        entries: Vec::new(), warnings: vec![
            "Coverage is limited to endpoint-reachable published cache history, not all refs; no fetch or index was performed.".into(),
            "Explicit revert references record commit-message declarations only; they do not verify patch inversion or selected-path reversal. Same-file material does not establish causality or stability. Subsequent rename continuity and changed-region overlap are not supported.".into(),
            "Cached merge diffs are relative to the first parent; branch correspondence is conservative, not proof of fresh corrections.".into(),
        ],
    };
    let mut states: HashMap<String, Incarnations> = HashMap::from([(seed.clone(), initial)]);
    let candidates: Vec<_> = graph
        .iter()
        .filter(|node| descendants.contains(&node.oid))
        .collect();
    let all_cached_oids = if candidates
        .iter()
        .any(|node| node.commit_time <= time_ceiling)
    {
        session.commit_oids()?
    } else {
        HashSet::new()
    };
    let mut explicit_reference_entries = Vec::new();
    let mut same_file_entries = Vec::new();
    for (index, node) in candidates.iter().enumerate() {
        let eligible = node.commit_time <= time_ceiling;
        if (eligible && report.inspected_count == max_commits)
            || (!eligible && report.lineage_inspected_count == max_commits)
        {
            report.traversal_truncated = candidates[index..]
                .iter()
                .any(|node| node.commit_time <= time_ceiling);
            report.warnings.push("Inspection stopped at the eligible or out-of-window lineage budget; uninspected history has no complete match count.".into());
            break;
        }
        if eligible {
            report.inspected_count += 1;
            report
                .inspected_first
                .get_or_insert_with(|| node.oid.clone());
            report.inspected_last = Some(node.oid.clone());
            if node.commit_time < seed_time {
                report.warnings.push(format!(
                    "Timestamp inversion at {}: signed elapsed {} seconds.",
                    node.oid,
                    node.commit_time - seed_time
                ));
            }
        } else {
            report.lineage_inspected_count += 1;
        }
        let has_revert_reference = if eligible {
            session.commit_message(&node.oid)?.is_some_and(|message| {
                explicitly_references_seed(&message, &seed, &all_cached_oids)
            })
        } else {
            false
        };
        let mut state = node
            .parents
            .first()
            .and_then(|parent| states.get(parent))
            .cloned()
            .unwrap_or_default();
        if state.is_empty() {
            report.warnings.push(format!("{}: first-parent file correspondence to the seed is unavailable; no same-file identity is assumed.", node.oid));
        }
        if node.parents.len() > 1 {
            for (path, active) in &mut state {
                if *active
                    && node
                        .parents
                        .iter()
                        .skip(1)
                        .any(|parent| states.get(parent).and_then(|s| s.get(path)) != Some(&true))
                {
                    *active = false;
                    report.warnings.push(format!(
                        "{}: ambiguous merge correspondence; tracking stopped for {}.",
                        node.oid,
                        String::from_utf8_lossy(path)
                    ));
                }
            }
        }
        let changes = session.forward_changes(&node.oid)?;
        let mut associated = Vec::new();
        let mut change_types = Vec::new();
        for change in changes {
            for (path, active) in &mut state {
                if !*active {
                    continue;
                }
                let old_match = change.old_path.as_ref() == Some(path);
                let new_match = change.new_path.as_ref() == Some(path);
                if !old_match && !new_match {
                    continue;
                }
                if change.status.starts_with('C') {
                    continue;
                }
                if change.status.starts_with('R') {
                    *active = false;
                    report.warnings.push(format!(
                        "{}: unsupported subsequent rename continuity; tracking stopped for {}.",
                        node.oid,
                        String::from_utf8_lossy(path)
                    ));
                    if !old_match {
                        continue;
                    }
                } else if change.status.starts_with('A') {
                    *active = false;
                    report.warnings.push(format!("{}: addition at a tracked path has no established incarnation continuity; tracking stopped.", node.oid));
                    continue;
                } else if change.status.starts_with('D') {
                    *active = false;
                }
                if eligible {
                    associated.push(path.clone());
                    change_types.push(change.status.clone());
                }
            }
        }
        states.insert(node.oid.clone(), state);
        let has_same_file_association = !associated.is_empty();
        if has_revert_reference || has_same_file_association {
            let mut combined: Vec<_> = associated.into_iter().zip(change_types).collect();
            combined.sort();
            combined.dedup();
            let (paths, change_types): (Vec<Vec<u8>>, Vec<String>) = combined.into_iter().unzip();
            let entry = Entry {
                commit_id: node.oid.clone(),
                subject: session.forward_subject(&node.oid)?,
                commit_time: node.commit_time,
                elapsed_seconds: node.commit_time - seed_time,
                paths,
                change_types,
                revert_reference: has_revert_reference.then(|| seed.clone()),
                same_file_association: has_same_file_association,
                parent_count: node.parents.len(),
                patch: None,
            };
            if has_revert_reference {
                explicit_reference_entries.push(entry);
            } else {
                same_file_entries.push(entry);
            }
        }
    }
    report.matched_in_inspected_scope = explicit_reference_entries.len() + same_file_entries.len();
    report.display_truncated = report.matched_in_inspected_scope > options.limit;
    for mut entry in explicit_reference_entries
        .into_iter()
        .chain(same_file_entries)
        .take(options.limit)
    {
        if options.patch {
            let include_commit_patch = entry.revert_reference.is_some();
            let paths = entry.paths.clone();
            entry.patch = Some(crate::analysis::patch::selected_patch_excerpt(
                &session,
                &entry.commit_id,
                |hunk| {
                    if include_commit_patch {
                        return Some(0);
                    }
                    [&hunk.old_path, &hunk.new_path]
                        .into_iter()
                        .flatten()
                        .any(|path| paths.contains(path))
                        .then_some(0)
                },
            )?);
        }
        report.entries.push(entry);
    }
    Ok(Outcome {
        progress: session.progress().to_vec(),
        warnings: session.warnings().to_vec(),
        report: QueryReport::Followups(report),
    })
}

fn explicitly_references_seed(
    message: &[u8],
    seed: &str,
    all_cached_oids: &HashSet<String>,
) -> bool {
    String::from_utf8_lossy(message).lines().any(|line| {
        let Some(reference) = line.trim().strip_prefix("This reverts commit ") else {
            return false;
        };
        let reference = reference.strip_suffix('.').unwrap_or(reference);
        if !reference.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return false;
        }
        crate::analysis::retrieval::resolve_oid_prefix(
            all_cached_oids,
            &reference.to_ascii_lowercase(),
        )
        .as_deref()
            == Some(seed)
    })
}

fn validate_path(path: &str) -> Result<(), AppError> {
    if path.is_empty()
        || path.contains('\0')
        || path.starts_with(['/', '\\'])
        || (path.as_bytes().get(1) == Some(&b':') && path.as_bytes()[0].is_ascii_alphabetic())
        || path
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(AppError::input(
            "--path must be an exact repository-relative file path",
        ));
    }
    Ok(())
}
