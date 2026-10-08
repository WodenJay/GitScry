//! Bounded same-file material. Identity is branch-local and never resumes after ending.
mod patches;
mod regions;

use crate::analysis::provenance::explicit_revert_declarations;
use crate::analysis::retrieval::single_revert_target;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::analysis::query::{Context, Options, Outcome, QueryReport, scope};
use crate::{app::AppError, cache::followups::ForwardCommit, git::Repository};
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

/// Typed per-commit diagnostic recorded at generation time; rendering derives
/// category summaries or the full warning string from it without parsing text.
pub(crate) struct Diagnostic {
    pub(crate) category: DiagnosticCategory,
    pub(crate) commit_id: String,
    /// False when the diagnostic arose during out-of-window lineage traversal.
    pub(crate) eligible: bool,
    /// Index into `Report::warnings` where this diagnostic was pushed.
    pub(crate) warning_index: usize,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum DiagnosticCategory {
    FirstParentFileCorrespondence,
    AmbiguousMergeFileCorrespondence,
    AmbiguousMergeRegionCorrespondence,
    TimestampInversion,
}

impl Report {
    /// Human-readable warnings. Verbose keeps every per-commit diagnostic in
    /// generation order; the default replaces dynamic diagnostics with
    /// fixed-order category summaries and one discovery hint. JSON keeps the
    /// full `warnings` array regardless of this choice.
    pub(crate) fn human_warnings(&self) -> std::borrow::Cow<'_, [String]> {
        if self.verbose {
            return std::borrow::Cow::Borrowed(&self.warnings);
        }
        std::borrow::Cow::Owned(self.aggregated_warnings())
    }

    fn aggregated_warnings(&self) -> Vec<String> {
        // Diagnostics record their exact `warnings` index at push time, so
        // stripping never depends on warning text.
        let diagnostic_indices: HashSet<usize> = self
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.warning_index)
            .collect();
        let mut warnings: Vec<String> = self
            .warnings
            .iter()
            .enumerate()
            .filter(|(index, _)| !diagnostic_indices.contains(index))
            .map(|(_, warning)| warning.clone())
            .collect();
        let mut summaries = Vec::new();
        for category in [
            DiagnosticCategory::FirstParentFileCorrespondence,
            DiagnosticCategory::AmbiguousMergeFileCorrespondence,
            DiagnosticCategory::AmbiguousMergeRegionCorrespondence,
            DiagnosticCategory::TimestampInversion,
        ] {
            summaries.extend(category.summaries(&self.diagnostics));
        }
        if !summaries.is_empty() {
            summaries.push("Use --verbose for per-commit diagnostics.".into());
        }
        warnings.extend(summaries);
        warnings
    }
}

/// Counts for one category over one inspection window. Commit-level categories
/// count distinct commits; file and region categories count file-tracking
/// instances (an affected seed file identity at one traversed commit) plus
/// distinct commits. Categories are never summed together.
#[derive(Default)]
struct CategoryCounts {
    inspected_instances: usize,
    inspected_commits: BTreeSet<String>,
    lineage_instances: usize,
    lineage_commits: BTreeSet<String>,
}

impl DiagnosticCategory {
    fn summaries(self, diagnostics: &[Diagnostic]) -> Vec<String> {
        let mut counts = CategoryCounts::default();
        for diagnostic in diagnostics.iter().filter(|d| d.category == self) {
            if diagnostic.eligible {
                counts.inspected_instances += 1;
                counts
                    .inspected_commits
                    .insert(diagnostic.commit_id.clone());
            } else {
                counts.lineage_instances += 1;
                counts.lineage_commits.insert(diagnostic.commit_id.clone());
            }
        }
        let mut summaries = Vec::new();
        if counts.inspected_instances > 0 {
            summaries.push(self.summary_line(
                counts.inspected_instances,
                counts.inspected_commits.len(),
                "inspected commit",
            ));
        }
        if counts.lineage_instances > 0 {
            summaries.push(self.summary_line(
                counts.lineage_instances,
                counts.lineage_commits.len(),
                "out-of-window lineage commit",
            ));
        }
        summaries
    }

    fn summary_line(self, instances: usize, commits: usize, commits_label: &str) -> String {
        let commits_label = plural(commits, commits_label);
        let instances_label = plural(instances, "file-tracking instance");
        let have = if commits == 1 { "has" } else { "have" };
        let lacks = if commits == 1 { "lacks" } else { "lack" };
        match self {
            Self::FirstParentFileCorrespondence => format!(
                "{commits} {commits_label} {lacks} first-parent file correspondence to the seed; no same-file identity is assumed."
            ),
            Self::AmbiguousMergeFileCorrespondence => format!(
                "{instances} {instances_label} across {commits} {commits_label} {have} ambiguous merge file correspondence; affected file tracking stopped."
            ),
            Self::AmbiguousMergeRegionCorrespondence => format!(
                "{instances} {instances_label} across {commits} {commits_label} {have} ambiguous merge region correspondence; changed-region tracking downgraded."
            ),
            Self::TimestampInversion => {
                format!("{commits} {commits_label} have commit times earlier than the seed time.")
            }
        }
    }
}

/// Pluralizes a singular noun phrase by appending `s` (e.g. "commit" ->
/// "commits", "file-tracking instance" -> "file-tracking instances").
fn plural(count: usize, singular: &str) -> String {
    if count == 1 {
        singular.to_owned()
    } else {
        format!("{singular}s")
    }
}

pub(crate) struct Report {
    pub(crate) verbose: bool,
    pub(crate) scope: Scope,
    pub(crate) inspected_count: usize,
    pub(crate) lineage_inspected_count: usize,
    pub(crate) inspected_first: Option<String>,
    pub(crate) inspected_last: Option<String>,
    pub(crate) traversal_truncated: bool,
    pub(crate) display_truncated: bool,
    pub(crate) matched_in_inspected_scope: usize,
    pub(crate) patch_matching: patches::MatchingReport,
    pub(crate) entries: Vec<Entry>,
    pub(crate) warnings: Vec<String>,
    pub(crate) diagnostics: Vec<Diagnostic>,
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
    pub(crate) patch_relationships: Vec<patches::Relationship>,
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
    verbose: bool,
    options: Options,
) -> Result<Outcome, AppError> {
    let seconds = i64::try_from(days)
        .ok()
        .and_then(|days| days.checked_mul(86400))
        .filter(|_| days > 0 && max_commits > 0)
        .ok_or_else(|| AppError::input("followups bounds must be positive and representable"))?;
    let paths = paths
        .into_iter()
        .map(|path| {
            let path = super::normalize_git_path_string(&path);
            validate_path(&path)?;
            Ok(path)
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    let repository = Repository::discover()?;
    let head = Context::pin_current_head(&repository)?;
    let seed = repository.resolve_commit(&revision)?;
    let requested_endpoint = to_rev
        .as_deref()
        .map(|revision| repository.resolve_commit(revision))
        .transpose()?;
    let session = crate::cache::refresh_query(&repository, &head)?;
    session.require_revision(&seed)?;
    let cache_tip = session.completed_tip()?;
    let (endpoint, coverage_complete) = match requested_endpoint {
        Some(endpoint) => {
            session.require_revision(&endpoint)?;
            let history = scope::reachable_history(&session, &repository, &endpoint)?;
            (endpoint, history.coverage_complete)
        }
        None => {
            let history = scope::reachable_history(&session, &repository, &head)?;
            let endpoint = history.revisions.first().cloned().ok_or_else(|| {
                AppError::input(
                    "no commits reachable from current HEAD are available after query refresh; run `gitscry index` to publish reachable history",
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
        "Coverage is limited to endpoint-reachable local history, not all refs; no fetch, initialization, repair, or upgrade was performed.".into(),
        "Explicit revert references are commit-message declarations only. Tracked regions begin at seed-added lines and continue only through complete cached text patches and detected renames; unrelated same-file edits remain same-file-only. Pure deletions establish no region; copies and same-path recreation do not continue an incarnation. Incomplete or ambiguous correspondence is downgraded. Neither association basis establishes causality or stability.".into(),
        "Cached merge diffs are relative to the first parent; region coordinates must agree across parents or tracking downgrades, and merge-imported work is not described as a fresh correction.".into(),
    ];
    if !coverage_complete {
        warnings.push(
            "Endpoint-reachable local history is incomplete; cached commits only are included. Run `gitscry index` after making additional history available."
                .into(),
        );
    }
    let candidates: Vec<_> = graph
        .iter()
        .filter(|node| descendants.contains(&node.oid))
        .collect();
    let eligible_count = candidates
        .iter()
        .filter(|node| node.commit_time <= time_ceiling)
        .count();
    let cache_candidates =
        eligible_candidates_within_inspection_budget(&candidates, max_commits, time_ceiling);
    let mut patch_matcher =
        patches::Matcher::new(&repository, &seed, &cache_candidates, eligible_count)?;
    let mut report = Report {
        verbose,
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
        patch_matching: patch_matcher.report(coverage_complete),
        entries: Vec::new(),
        warnings,
        diagnostics: Vec::new(),
    };
    let mut states: HashMap<String, Incarnations> = HashMap::from([(seed.clone(), initial)]);
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
    let mut patch_relationship_entries = Vec::new();
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
                push_diagnostic(
                    &mut report,
                    DiagnosticCategory::TimestampInversion,
                    &node.oid,
                    eligible,
                    format!(
                        "Timestamp inversion at {}: signed elapsed {} seconds.",
                        node.oid,
                        node.commit_time - seed_time
                    ),
                );
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
        let patch_relationship = if eligible {
            patch_matcher.check(&node.oid)?
        } else {
            None
        };
        let mut state = node
            .parents
            .first()
            .and_then(|parent| states.get(parent))
            .cloned()
            .unwrap_or_default();
        if state.is_empty() {
            push_diagnostic(
                &mut report,
                DiagnosticCategory::FirstParentFileCorrespondence,
                &node.oid,
                eligible,
                format!(
                    "{}: first-parent file correspondence to the seed is unavailable; no same-file identity is assumed.",
                    node.oid
                ),
            );
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
                        push_diagnostic(
                            &mut report,
                            DiagnosticCategory::AmbiguousMergeRegionCorrespondence,
                            &node.oid,
                            eligible,
                            format!(
                                "{}: ambiguous merge region correspondence; changed-region tracking downgraded for {}.",
                                node.oid,
                                String::from_utf8_lossy(
                                    parent_paths[0].as_ref().expect("checked path")
                                )
                            ),
                        );
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
                push_diagnostic(
                    &mut report,
                    DiagnosticCategory::AmbiguousMergeFileCorrespondence,
                    &node.oid,
                    eligible,
                    format!(
                        "{}: ambiguous merge file correspondence; file tracking stopped for {}.",
                        node.oid,
                        String::from_utf8_lossy(&path)
                    ),
                );
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
        if has_revert_reference || has_same_file_association || patch_relationship.is_some() {
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
                patch_relationships: patch_relationship.into_iter().collect(),
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
            } else if has_same_file_association {
                same_file_entries.push(entry);
            } else {
                patch_relationship_entries.push(entry);
            }
        }
    }
    report.patch_matching = patch_matcher.report(coverage_complete);
    report.matched_in_inspected_scope = explicit_reference_entries.len()
        + region_overlap_entries.len()
        + same_file_entries.len()
        + patch_relationship_entries.len();
    report.display_truncated = report.matched_in_inspected_scope > options.limit;
    for mut entry in explicit_reference_entries
        .into_iter()
        .chain(region_overlap_entries)
        .chain(same_file_entries)
        .chain(patch_relationship_entries)
        .take(options.limit)
    {
        if options.patch {
            let include_commit_patch = entry.revert_reference.is_some();
            let mut paths = entry.paths.clone();
            paths.extend(
                entry
                    .patch_relationships
                    .iter()
                    .flat_map(|relationship| relationship.paths.iter().cloned()),
            );
            paths.sort();
            paths.dedup();
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

fn push_diagnostic(
    report: &mut Report,
    category: DiagnosticCategory,
    commit_id: &str,
    eligible: bool,
    warning: String,
) {
    report.diagnostics.push(Diagnostic {
        category,
        commit_id: commit_id.to_owned(),
        eligible,
        warning_index: report.warnings.len(),
    });
    report.warnings.push(warning);
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
    let text = String::from_utf8_lossy(message);
    // A declaration naming several different commits identifies nothing, so the seed is
    // referenced only when the message resolves to the seed alone.
    single_revert_target(explicit_revert_declarations(&text), |hex| {
        crate::analysis::retrieval::resolve_oid_prefix(all_cached_oids, hex)
    })
    .is_some_and(|target| target == seed)
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

fn eligible_candidates_within_inspection_budget(
    candidates: &[&ForwardCommit],
    max_commits: usize,
    time_ceiling: i64,
) -> Vec<String> {
    let (mut eligible_count, mut lineage_count) = (0, 0);
    let mut inspected = Vec::new();
    for node in candidates {
        let eligible = node.commit_time <= time_ceiling;
        if (eligible && eligible_count == max_commits)
            || (!eligible && lineage_count == max_commits)
        {
            break;
        }
        if eligible {
            eligible_count += 1;
            inspected.push(node.oid.clone());
        } else {
            lineage_count += 1;
        }
    }
    inspected
}

#[cfg(test)]
mod tests {
    use super::{ForwardCommit, eligible_candidates_within_inspection_budget};

    fn commit(oid: &str, commit_time: i64) -> ForwardCommit {
        ForwardCommit {
            oid: oid.to_owned(),
            commit_time,
            parents: Vec::new(),
        }
    }

    #[test]
    fn patch_candidates_stop_at_eligible_and_lineage_budgets() {
        let late_one = commit("late-one", 101);
        let late_two = commit("late-two", 102);
        let late_three = commit("late-three", 103);
        let eligible_before_lineage_limit = commit("eligible-before-limit", 10);
        let eligible_after_lineage_limit = commit("eligible-after-limit", 20);
        assert_eq!(
            eligible_candidates_within_inspection_budget(
                &[
                    &late_one,
                    &late_two,
                    &eligible_before_lineage_limit,
                    &late_three,
                    &eligible_after_lineage_limit,
                ],
                2,
                100,
            ),
            vec!["eligible-before-limit"]
        );

        let eligible_one = commit("eligible-one", 10);
        let eligible_two = commit("eligible-two", 20);
        let eligible_three = commit("eligible-three", 30);
        assert_eq!(
            eligible_candidates_within_inspection_budget(
                &[
                    &late_one,
                    &eligible_one,
                    &late_two,
                    &eligible_two,
                    &eligible_three
                ],
                2,
                100,
            ),
            vec!["eligible-one", "eligible-two"]
        );
    }
}
