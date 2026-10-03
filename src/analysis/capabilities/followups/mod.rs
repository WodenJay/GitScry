//! Bounded same-file material. Identity is branch-local and never resumes after ending.
mod regions;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::analysis::query::{Options, Outcome, QueryReport, scope};
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

type Incarnations = BTreeMap<i64, FileIncarnation>;

#[derive(Clone, Eq, PartialEq)]
struct FileIncarnation {
    seed_old_path: Option<Vec<u8>>,
    seed_new_path: Option<Vec<u8>>,
    current_path: Option<Vec<u8>>,
    regions: RegionTracking,
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct FileAssociation {
    pub(crate) seed_old_path: Option<Vec<u8>>,
    pub(crate) seed_new_path: Option<Vec<u8>>,
    pub(crate) previous_path: Vec<u8>,
    pub(crate) current_path: Option<Vec<u8>>,
    pub(crate) change_type: String,
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinePosition {
    pub(crate) start_line: i64,
    pub(crate) line_count: i64,
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct RegionAssociation {
    pub(crate) seed_path: Vec<u8>,
    pub(crate) previous_revision: String,
    pub(crate) previous_path: Vec<u8>,
    pub(crate) current_path: Option<Vec<u8>>,
    pub(crate) seed_position: LinePosition,
    pub(crate) previous_position: LinePosition,
    pub(crate) current_position: LinePosition,
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct RegionDowngrade {
    pub(crate) seed_old_path: Option<Vec<u8>>,
    pub(crate) seed_new_path: Option<Vec<u8>>,
    pub(crate) previous_path: Vec<u8>,
    pub(crate) current_path: Option<Vec<u8>>,
    pub(crate) reason: RegionTrackingReason,
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum RegionTrackingReason {
    MissingPatchMaterial,
    TruncatedPatchMaterial,
    TrackingBudgetExhausted,
    NonTextualSeedChange,
    NonTextualChange,
    InvalidPatchMapping,
    AmbiguousMerge,
}

impl RegionTrackingReason {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::MissingPatchMaterial => "missing_patch_material",
            Self::TruncatedPatchMaterial => "truncated_patch_material",
            Self::TrackingBudgetExhausted => "tracking_budget_exhausted",
            Self::NonTextualSeedChange => "non_textual_seed_change",
            Self::NonTextualChange => "non_textual_change",
            Self::InvalidPatchMapping => "invalid_patch_mapping",
            Self::AmbiguousMerge => "ambiguous_merge_correspondence",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::MissingPatchMaterial => {
                "cached patch objects needed to trace this region are missing"
            }
            Self::TruncatedPatchMaterial => "cached patch text or hunk ranges are truncated",
            Self::TrackingBudgetExhausted => {
                "the bounded changed-region tracking budget was exhausted"
            }
            Self::NonTextualSeedChange => {
                "the seed change has no cached textual hunk to establish added-line regions"
            }
            Self::NonTextualChange => {
                "the later file change has no cached textual hunk to map tracked lines"
            }
            Self::InvalidPatchMapping => "cached patch hunk ranges could not be mapped reliably",
            Self::AmbiguousMerge => "parent branches disagree on changed-region coordinates",
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
enum RegionTracking {
    Available(Vec<TrackedRegion>),
    Unavailable(RegionTrackingReason),
}

#[derive(Clone, Eq, PartialEq)]
struct TrackedRegion {
    seed_position: LinePosition,
    current_positions: Vec<LinePosition>,
    protected_boundaries: Vec<i64>,
}

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
    pub(crate) file_associations: Vec<FileAssociation>,
    pub(crate) revert_reference: Option<String>,
    pub(crate) same_file_association: bool,
    pub(crate) region_associations: Vec<RegionAssociation>,
    pub(crate) region_tracking_downgrades: Vec<RegionDowngrade>,
    pub(crate) parent_count: usize,
    pub(crate) patch: Option<crate::analysis::PatchExcerpt>,
}

pub(in crate::analysis) fn run(
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
    let (endpoint, coverage_complete) = match to_rev {
        Some(revision) => {
            let endpoint = repository.resolve_commit(&revision)?;
            session.require_revision(&endpoint)?;
            let history = scope::reachable_history(&session, &repository, &endpoint)?;
            (endpoint, history.coverage_complete)
        }
        None => {
            let head = repository.resolve_commit("HEAD")?;
            let history = scope::reachable_history(&session, &repository, &head)?;
            let endpoint = history.revisions.first().cloned().ok_or_else(|| {
                AppError::input(
                    "no commits reachable from current HEAD are present in the published cache; run `gitscry index` first",
                )
            })?;
            (endpoint, history.coverage_complete)
        }
    };
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
            selection_matches(
                change.old_path.as_deref(),
                change.new_path.as_deref(),
                path.as_bytes(),
            )
        }) {
            return Err(AppError::input(format!(
                "path was not changed by seed revision: {path}"
            )));
        }
    }
    let mut initial = Incarnations::new();
    let mut region_budget = regions::Budget::new();
    let mut selected_paths = Vec::new();
    for change in &original {
        if !paths.is_empty()
            && !paths.iter().any(|path| {
                selection_matches(
                    change.old_path.as_deref(),
                    change.new_path.as_deref(),
                    path.as_bytes(),
                )
            })
        {
            continue;
        }
        for path in [&change.old_path, &change.new_path].into_iter().flatten() {
            selected_paths.push(path.clone());
        }
        initial.insert(
            change.ordinal,
            FileIncarnation {
                seed_old_path: change.old_path.clone(),
                seed_new_path: change.new_path.clone(),
                current_path: if change.status.starts_with('D') {
                    None
                } else {
                    change.new_path.clone().or_else(|| change.old_path.clone())
                },
                regions: regions::seed_tracking(
                    &session,
                    &seed,
                    change.ordinal,
                    change.old_blob != change.new_blob,
                    change.status.starts_with('D'),
                    &mut region_budget,
                )?,
            },
        );
    }
    selected_paths.sort();
    selected_paths.dedup();

    let mut warnings = vec![
        "Coverage is limited to endpoint-reachable published cache history, not all refs; no fetch or index was performed.".into(),
        "Explicit revert references are commit-message declarations only. Tracked regions begin at seed-added lines and continue only through complete cached text patches and detected renames; unrelated same-file edits remain same-file-only. Pure deletions establish no region; copies and same-path recreation do not continue an incarnation. Incomplete or ambiguous correspondence is downgraded. Neither association basis establishes causality or stability.".into(),
        "Cached merge diffs are relative to the first parent; region coordinates must agree across parents or tracking downgrades, and merge-imported work is not described as a fresh correction.".into(),
    ];
    if !coverage_complete {
        warnings.push(
            "Endpoint-reachable local history is incomplete; cached commits only are included. Run `gitscry index` after making additional history available."
                .into(),
        );
    }
    let mut report = Report {
        scope: Scope {
            seed: seed.clone(),
            endpoint,
            cache_tip,
            seed_time,
            time_ceiling,
            days,
            max_commits,
            limit: options.limit,
            selected_paths,
        },
        inspected_count: 0,
        lineage_inspected_count: 0,
        inspected_first: None,
        inspected_last: None,
        traversal_truncated: false,
        display_truncated: false,
        matched_in_inspected_scope: 0,
        entries: Vec::new(),
        warnings,
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
    let mut region_overlap_entries = Vec::new();
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
            let mut identities: BTreeSet<_> = state.keys().copied().collect();
            for parent in node.parents.iter().skip(1) {
                if let Some(parent_state) = states.get(parent) {
                    identities.extend(parent_state.keys().copied());
                }
            }
            for identity in identities {
                let parent_paths: Vec<_> = node
                    .parents
                    .iter()
                    .map(|parent| {
                        states
                            .get(parent)
                            .and_then(|parent_state| parent_state.get(&identity))
                            .and_then(|incarnation| incarnation.current_path.clone())
                    })
                    .collect();
                let parent_regions: Vec<_> = node
                    .parents
                    .iter()
                    .map(|parent| {
                        states
                            .get(parent)
                            .and_then(|parent_state| parent_state.get(&identity))
                            .map(|incarnation| incarnation.regions.clone())
                    })
                    .collect();
                if parent_paths
                    .iter()
                    .skip(1)
                    .all(|path| path == &parent_paths[0])
                {
                    if parent_paths[0].is_some()
                        && parent_regions
                            .iter()
                            .skip(1)
                            .any(|regions| regions != &parent_regions[0])
                    {
                        if let Some(incarnation) = state.get_mut(&identity) {
                            incarnation.regions =
                                RegionTracking::Unavailable(RegionTrackingReason::AmbiguousMerge);
                        }
                        report.warnings.push(format!(
                            "{}: ambiguous merge region correspondence; changed-region tracking downgraded for {}.",
                            node.oid,
                            String::from_utf8_lossy(parent_paths[0].as_ref().expect("checked path"))
                        ));
                    }
                    continue;
                }
                let Some(path) = parent_paths.iter().find_map(|path| path.clone()) else {
                    continue;
                };
                if let Some(incarnation) = state.get_mut(&identity) {
                    incarnation.current_path = None;
                    incarnation.regions =
                        RegionTracking::Unavailable(RegionTrackingReason::AmbiguousMerge);
                }
                report.warnings.push(format!(
                    "{}: ambiguous merge file correspondence; file tracking stopped for {}.",
                    node.oid,
                    String::from_utf8_lossy(&path)
                ));
            }
        }
        let changes = session.forward_changes(&node.oid)?;
        let mut associated = Vec::new();
        let mut region_associations = Vec::new();
        let mut region_tracking_downgrades = Vec::new();
        let mut next_state = state.clone();
        let previous_revision = node.parents.first().map(String::as_str).unwrap_or(&seed);
        for change in changes {
            if change.status.starts_with('C') {
                continue;
            }
            for (identity, incarnation) in &state {
                let Some(previous_path) = incarnation.current_path.as_ref() else {
                    continue;
                };
                if change.old_path.as_ref() != Some(previous_path) {
                    continue;
                }
                let previous_path = previous_path.clone();
                let current_path = if change.status.starts_with('D') {
                    None
                } else {
                    change
                        .new_path
                        .clone()
                        .or_else(|| Some(previous_path.clone()))
                };
                let advance = regions::advance(
                    &session,
                    &incarnation.regions,
                    regions::ChangeContext {
                        oid: &node.oid,
                        change_ordinal: change.ordinal,
                        content_changed: change.old_blob != change.new_blob,
                        previous_revision,
                        seed_path: incarnation
                            .seed_new_path
                            .as_deref()
                            .or(incarnation.seed_old_path.as_deref()),
                        previous_path: &previous_path,
                        current_path: current_path.as_deref(),
                    },
                    eligible,
                    &mut region_budget,
                )?;
                if eligible {
                    region_associations.extend(advance.associations);
                    if let Some(reason) = advance.downgrade_reason {
                        region_tracking_downgrades.push(RegionDowngrade {
                            seed_old_path: incarnation.seed_old_path.clone(),
                            seed_new_path: incarnation.seed_new_path.clone(),
                            previous_path: previous_path.clone(),
                            current_path: current_path.clone(),
                            reason,
                        });
                    }
                    associated.push(FileAssociation {
                        seed_old_path: incarnation.seed_old_path.clone(),
                        seed_new_path: incarnation.seed_new_path.clone(),
                        previous_path: previous_path.clone(),
                        current_path: current_path.clone(),
                        change_type: change.status.clone(),
                    });
                }
                if let Some(next_incarnation) = next_state.get_mut(identity) {
                    next_incarnation.current_path = current_path;
                    next_incarnation.regions = advance.tracking;
                }
            }
        }
        state = next_state;
        states.insert(node.oid.clone(), state);
        let has_same_file_association = !associated.is_empty();
        let has_region_overlap = !region_associations.is_empty();
        if has_revert_reference || has_same_file_association {
            let mut file_associations = associated;
            file_associations.sort();
            file_associations.dedup();
            region_associations.sort();
            region_associations.dedup();
            region_tracking_downgrades.sort();
            region_tracking_downgrades.dedup();
            let mut paths: Vec<_> = file_associations
                .iter()
                .flat_map(|association| {
                    std::iter::once(association.previous_path.clone())
                        .chain(association.current_path.iter().cloned())
                })
                .collect();
            paths.sort();
            paths.dedup();
            let mut change_types: Vec<_> = file_associations
                .iter()
                .map(|association| association.change_type.clone())
                .collect();
            change_types.sort();
            change_types.dedup();
            let entry = Entry {
                commit_id: node.oid.clone(),
                subject: session.forward_subject(&node.oid)?,
                commit_time: node.commit_time,
                elapsed_seconds: node.commit_time - seed_time,
                paths,
                change_types,
                file_associations,
                revert_reference: has_revert_reference.then(|| seed.clone()),
                same_file_association: has_same_file_association,
                region_associations,
                region_tracking_downgrades,
                parent_count: node.parents.len(),
                patch: None,
            };
            if has_revert_reference {
                explicit_reference_entries.push(entry);
            } else if has_region_overlap {
                region_overlap_entries.push(entry);
            } else {
                same_file_entries.push(entry);
            }
        }
    }
    report.matched_in_inspected_scope =
        explicit_reference_entries.len() + region_overlap_entries.len() + same_file_entries.len();
    report.display_truncated = report.matched_in_inspected_scope > options.limit;
    for mut entry in explicit_reference_entries
        .into_iter()
        .chain(region_overlap_entries)
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

fn selection_matches(
    old_path: Option<&[u8]>,
    new_path: Option<&[u8]>,
    selected_path: &[u8],
) -> bool {
    old_path == Some(selected_path) || new_path == Some(selected_path)
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
