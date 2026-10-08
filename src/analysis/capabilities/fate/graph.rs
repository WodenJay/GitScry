use std::collections::{BTreeSet, HashMap, HashSet};

use crate::cache::followups::ForwardCommit;
use crate::git::{Change, FateTarget, Hunk, Repository};
use crate::{
    analysis::query::{Context, Options, Outcome, QueryReport},
    app::AppError,
    cache::QuerySession,
};

use super::{Association, Event, FinalState, Location, Report, StopReason, SymbolSelection};

#[derive(Clone)]
struct Reason {
    code: &'static str,
    explanation: &'static str,
}

#[derive(Clone)]
enum Status {
    Active {
        location: Location,
        span: Option<(i64, i64)>,
    },
    Deleted {
        location: Option<Location>,
        stopped_at: String,
        reason: Reason,
    },
    Unknown {
        location: Option<Location>,
        stopped_at: String,
        reason: Reason,
    },
}

#[derive(Clone)]
struct BranchState {
    status: Status,
    heads: BTreeSet<String>,
}

impl BranchState {
    fn location(&self) -> Option<&Location> {
        match &self.status {
            Status::Active { location, .. } => Some(location),
            Status::Deleted { location, .. } | Status::Unknown { location, .. } => {
                location.as_ref()
            }
        }
    }

    fn span(&self) -> Option<(i64, i64)> {
        match self.status {
            Status::Active { span, .. } => span,
            _ => None,
        }
    }

    fn unknown(
        location: Option<Location>,
        stopped_at: String,
        code: &'static str,
        explanation: &'static str,
    ) -> Self {
        Self {
            status: Status::Unknown {
                location,
                stopped_at,
                reason: Reason { code, explanation },
            },
            heads: BTreeSet::new(),
        }
    }
}

#[derive(Default)]
struct EventGraph {
    events: Vec<Event>,
    indexes: HashMap<String, usize>,
}

impl EventGraph {
    fn push(&mut self, mut event: Event) {
        event.kinds.sort_unstable();
        event.kinds.dedup();
        event.predecessors.sort_unstable();
        event.predecessors.dedup();
        event.incoming.sort();

        if let Some(index) = self.indexes.get(&event.commit_id).copied() {
            let current = &mut self.events[index];
            current.result_state = event.result_state;
            for kind in event.kinds {
                if !current.kinds.contains(&kind) {
                    current.kinds.push(kind);
                }
            }
            for predecessor in event.predecessors {
                if !current.predecessors.contains(&predecessor) {
                    current.predecessors.push(predecessor);
                }
            }
            for incoming in &event.incoming {
                let event_count = event
                    .incoming
                    .iter()
                    .filter(|candidate| *candidate == incoming)
                    .count();
                let current_count = current
                    .incoming
                    .iter()
                    .filter(|candidate| *candidate == incoming)
                    .count();
                if current_count < event_count {
                    current.incoming.push(incoming.clone());
                }
            }
            if event.after.is_some() {
                current.after = event.after;
                current.after_span = event.after_span;
            }
            if current.patch.is_none() {
                current.patch = event.patch;
            }
            current.kinds.sort_unstable();
            current.predecessors.sort_unstable();
            current.incoming.sort();
        } else {
            self.indexes
                .insert(event.commit_id.clone(), self.events.len());
            self.events.push(event);
        }
    }
}

/// Attach the cached first-parent patch for the merge's first relevant input.
fn merge_event_patch(
    session: &QuerySession,
    include_patch: bool,
    commit_id: &str,
    incoming: &[BranchState],
) -> Result<Option<super::PatchExcerpt>, AppError> {
    if !include_patch {
        return Ok(None);
    }
    let changes = session.forward_changes(commit_id)?;
    let selected = changes
        .iter()
        .filter(|change| !change.status.starts_with('C'))
        .find_map(|change| {
            incoming
                .iter()
                .filter_map(BranchState::location)
                .find(|before| change.old_path.as_ref() == Some(&before.path))
                .map(|before| (change, before))
        });
    let Some((change, before)) = selected else {
        return Ok(None);
    };
    super::event_patch(session, true, commit_id, change.ordinal, before.line)
}

pub(super) struct GraphQuery<'a> {
    pub(super) session: &'a QuerySession,
    pub(super) repository: &'a Repository,
    pub(super) context: &'a Context,
    pub(super) target: FateTarget,
    pub(super) endpoint: String,
    pub(super) endpoint_frontier: bool,
    pub(super) max_commits: Option<usize>,
    pub(super) options: Options,
    pub(super) warnings: Vec<String>,
    pub(super) graph: &'a [ForwardCommit],
    pub(super) tracked: &'a HashSet<String>,
}

pub(super) fn execute(query: GraphQuery<'_>) -> Result<Outcome, AppError> {
    let GraphQuery {
        session,
        repository,
        context,
        target,
        endpoint,
        endpoint_frontier,
        max_commits,
        options,
        mut warnings,
        graph,
        tracked,
    } = query;
    let selection = target.symbol_selection.clone();
    let start_revision = target.revision.clone();
    let start_location = Location {
        path: target.path.clone(),
        line: i64::try_from(target.line)
            .map_err(|_| AppError::input("line does not fit the tracked coordinate space"))?,
    };
    let initial = BranchState {
        status: Status::Active {
            location: start_location,
            span: selection.as_ref().map(|_| {
                (
                    i64::try_from(target.line).unwrap_or(0),
                    i64::try_from(target.symbol_end_line.unwrap_or(target.line)).unwrap_or(0),
                )
            }),
        },
        heads: BTreeSet::from([start_revision.clone()]),
    };
    let ordered = graph
        .iter()
        .filter(|node| tracked.contains(&node.oid))
        .collect::<Vec<_>>();
    let ceiling = max_commits.unwrap_or(usize::MAX);
    let allowed_nodes = ordered.iter().take(ceiling).copied().collect::<Vec<_>>();
    let allowed = allowed_nodes
        .iter()
        .map(|node| node.oid.clone())
        .collect::<HashSet<_>>();
    let by_oid = graph
        .iter()
        .map(|node| (node.oid.as_str(), node))
        .collect::<HashMap<_, _>>();
    let ranks = allowed_nodes
        .iter()
        .enumerate()
        .map(|(rank, node)| (node.oid.as_str(), rank))
        .collect::<HashMap<_, _>>();
    let truncated = ordered.len() > allowed_nodes.len();
    let mut merge_states = HashMap::<String, BranchState>::new();
    let mut state_cache = HashMap::<String, BranchState>::new();
    let mut events = EventGraph::default();

    for node in allowed_nodes
        .iter()
        .copied()
        .filter(|node| node.parents.len() > 1)
    {
        let mut incoming = Vec::new();
        let mut outcomes = Vec::new();
        for parent in &node.parents {
            let Some(state) = state_at(
                parent,
                &start_revision,
                &initial,
                session,
                repository,
                context,
                selection.as_ref(),
                &by_oid,
                &allowed,
                &merge_states,
                &mut state_cache,
                &mut events,
                &mut warnings,
                &options,
            )?
            else {
                continue;
            };
            incoming.push(state.clone());
            outcomes.push(merge_parent_state(
                repository,
                parent,
                &node.oid,
                &state,
                selection.as_ref(),
            ));
        }
        if incoming.is_empty() {
            continue;
        }
        let (status, after, after_span) = reconcile(&outcomes, node.oid.as_str());
        let mut predecessors = BTreeSet::new();
        for state in &incoming {
            predecessors.extend(state.heads.iter().cloned());
        }
        let mut incoming_locations = incoming
            .iter()
            .filter_map(|state| state.location().cloned())
            .collect::<Vec<_>>();
        incoming_locations.sort();
        let before = incoming_locations
            .first()
            .cloned()
            .unwrap_or_else(|| Location {
                path: target.path.clone(),
                line: i64::try_from(target.line).unwrap_or(0),
            });

        let patch = merge_event_patch(session, options.patch, &node.oid, &incoming)?;
        events.push(Event {
            commit_id: node.oid.clone(),
            subject: session.forward_subject(&node.oid)?,
            commit_time: node.commit_time,
            relationship: "merge",
            result_state: match &status {
                Status::Active { .. } => "active",
                Status::Deleted { .. } => "deleted",
                Status::Unknown { .. } => "unknown",
            },
            kinds: vec!["merge"],
            predecessors: predecessors.iter().cloned().collect(),
            incoming: incoming_locations,
            before,
            after,
            before_span: incoming.iter().find_map(BranchState::span),
            after_span,
            parent_count: node.parents.len(),
            patch,
        });
        let mut heads = BTreeSet::from([node.oid.clone()]);
        if matches!(&status, Status::Unknown { .. }) && predecessors.is_empty() {
            heads.clear();
        }
        merge_states.insert(node.oid.clone(), BranchState { status, heads });
        state_cache.clear();
    }

    let mut final_state = if let Some(state) = merge_states.get(&endpoint) {
        state.clone()
    } else if allowed.contains(&endpoint) {
        state_at(
            &endpoint,
            &start_revision,
            &initial,
            session,
            repository,
            context,
            selection.as_ref(),
            &by_oid,
            &allowed,
            &merge_states,
            &mut state_cache,
            &mut events,
            &mut warnings,
            &options,
        )?
        .unwrap_or_else(|| initial.clone())
    } else {
        initial.clone()
    };
    let (mut stopped_at, mut stop_reason) = status_stop(&final_state.status);
    if truncated {
        let last = allowed_nodes
            .last()
            .map(|node| node.oid.clone())
            .unwrap_or_else(|| start_revision.clone());
        final_state = if let Some(state) = merge_states.get(&last) {
            state.clone()
        } else if allowed.contains(&last) {
            state_at(
                &last,
                &start_revision,
                &initial,
                session,
                repository,
                context,
                selection.as_ref(),
                &by_oid,
                &allowed,
                &merge_states,
                &mut state_cache,
                &mut events,
                &mut warnings,
                &options,
            )?
            .unwrap_or(final_state)
        } else {
            final_state
        };
        stopped_at = Some(last);
        stop_reason = Some(Reason {
            code: "max_commits_exhausted",
            explanation: "the --max-commits inspection budget was exhausted before the endpoint",
        });
        push_warning(
            &mut warnings,
            "Inspection stopped at the --max-commits budget; uninspected forward commits are not accounted for.",
        );
    } else if endpoint_frontier {
        stopped_at = Some(endpoint.clone());
        stop_reason = Some(Reason {
            code: "endpoint_not_covered",
            explanation: "cached coverage ends before the requested endpoint; survival to the endpoint is not established",
        });
    }

    events.events.sort_by_key(|event| {
        ranks
            .get(event.commit_id.as_str())
            .copied()
            .unwrap_or(usize::MAX)
    });
    let total_events = events.events.len();
    let display_truncated = total_events > options.limit;
    let displayed = events
        .events
        .into_iter()
        .rev()
        .take(options.limit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let final_code = if truncated || endpoint_frontier {
        FinalState::Unknown
    } else {
        match final_state.status {
            Status::Active { .. } => FinalState::ReachedEndpoint,
            Status::Deleted { .. } => FinalState::Deleted,
            Status::Unknown { .. } => FinalState::Unknown,
        }
    };
    let stop_reason = stop_reason.map(|reason| StopReason {
        code: reason.code,
        explanation: reason.explanation,
    });
    Ok(super::finished(
        session,
        Report {
            start_revision,
            endpoint,
            path: target.path,
            line: target.line,
            symbol_selection: selection,
            final_state: final_code,
            stopped_at,
            stop_reason,
            associations: Vec::<Association>::new(),
            associations_truncated: false,
            last_location: final_state.location().cloned(),
            last_span: final_state.span(),
            inspected_commits: allowed_nodes.len(),
            max_commits,
            traversal_truncated: truncated,
            total_events,
            display_truncated,
            limit: options.limit,
            events: displayed,
            scope: None,
        },
        warnings,
    ))
}

#[allow(clippy::too_many_arguments)]
fn state_at(
    revision: &str,
    start_revision: &str,
    initial: &BranchState,
    session: &QuerySession,
    repository: &Repository,
    context: &Context,
    selection: Option<&SymbolSelection>,
    by_oid: &HashMap<&str, &ForwardCommit>,
    allowed: &HashSet<String>,
    merge_states: &HashMap<String, BranchState>,
    cache: &mut HashMap<String, BranchState>,
    events: &mut EventGraph,
    warnings: &mut Vec<String>,
    options: &Options,
) -> Result<Option<BranchState>, AppError> {
    if revision == start_revision {
        return Ok(Some(initial.clone()));
    }
    if let Some(state) = merge_states.get(revision).or_else(|| cache.get(revision)) {
        return Ok(Some(state.clone()));
    }
    if !allowed.contains(revision) {
        return Ok(None);
    }
    let mut cursor = revision.to_owned();
    let base_revision;
    let base_state;
    loop {
        if cursor == start_revision {
            base_revision = cursor;
            base_state = initial.clone();
            break;
        }
        if let Some(state) = merge_states.get(&cursor) {
            base_revision = cursor;
            base_state = state.clone();
            break;
        }
        if let Some(state) = cache.get(&cursor) {
            base_revision = cursor;
            base_state = state.clone();
            break;
        }
        if !allowed.contains(&cursor) {
            return Ok(None);
        }
        let Some(node) = by_oid.get(cursor.as_str()) else {
            return Ok(None);
        };
        if node.parents.len() != 1 {
            let unknown = BranchState::unknown(
                None,
                cursor,
                "discontinuous_correspondence",
                "an earlier merge has no confirmed correspondence from the starting target",
            );
            cache.insert(revision.to_owned(), unknown.clone());
            return Ok(Some(unknown));
        }
        cursor = node.parents[0].clone();
    }
    if revision == base_revision {
        return Ok(Some(base_state));
    }
    if !matches!(&base_state.status, Status::Active { .. }) {
        cache.insert(revision.to_owned(), base_state.clone());
        return Ok(Some(base_state));
    }
    let Status::Active { location, .. } = &base_state.status else {
        unreachable!("non-active states return above")
    };
    let location = location.clone();
    let Ok(path) = std::str::from_utf8(&location.path) else {
        let state = BranchState::unknown(
            Some(location),
            revision.to_owned(),
            "symbol_source_unavailable",
            "the tracked path is not UTF-8 and cannot be re-pinned for branch traversal",
        );
        cache.insert(revision.to_owned(), state.clone());
        return Ok(Some(state));
    };
    let target = match selection {
        Some(selection) => {
            repository.pin_fate_target(&base_revision, path, None, Some(&selection.qualified_name))
        }
        None => repository.pin_fate_target(
            &base_revision,
            path,
            usize::try_from(location.line).ok(),
            None,
        ),
    };
    let Ok(target) = target else {
        let state = BranchState::unknown(
            Some(location),
            revision.to_owned(),
            "symbol_source_unavailable",
            "the tracked target could not be revalidated at the branch base",
        );
        cache.insert(revision.to_owned(), state.clone());
        return Ok(Some(state));
    };
    let segment_options = Options {
        limit: usize::MAX,
        patch: options.patch,
        scope: options.scope.clone(),
    };
    let outcome = if selection.is_some() {
        super::execute_symbol(
            session,
            repository,
            context,
            target,
            Some(revision.to_owned()),
            None,
            segment_options,
        )?
    } else {
        super::execute_line(
            session,
            repository,
            context,
            target,
            Some(revision.to_owned()),
            None,
            segment_options,
        )?
    };
    for warning in &outcome.warnings {
        if !warnings.contains(warning) {
            warnings.push(warning.clone());
        }
    }
    let QueryReport::Fate(mut report) = outcome.report else {
        unreachable!("fate segment returned a different report type")
    };
    let mut heads = base_state.heads.clone();
    let mut current_commit: Option<String> = None;
    let mut current_predecessors = heads.clone();
    for mut event in report.events.drain(..) {
        if current_commit.as_deref() != Some(event.commit_id.as_str()) {
            current_commit = Some(event.commit_id.clone());
            current_predecessors = heads.clone();
            heads = BTreeSet::from([event.commit_id.clone()]);
        }
        event.predecessors = current_predecessors.iter().cloned().collect();
        events.push(event);
    }
    let state = state_from_report(report, heads, revision);
    cache.insert(revision.to_owned(), state.clone());
    Ok(Some(state))
}

fn state_from_report(report: Report, heads: BTreeSet<String>, revision: &str) -> BranchState {
    let location = report.last_location;
    let status = match report.final_state {
        FinalState::ReachedEndpoint => location
            .clone()
            .map(|location| Status::Active {
                location,
                span: report.last_span,
            })
            .unwrap_or_else(|| Status::unknown(
                None,
                revision.to_owned(),
                "invalid_patch_mapping",
                "the tracked location was unavailable at the branch endpoint",
            )),
        FinalState::Deleted => Status::Deleted {
            location,
            stopped_at: report.stopped_at.unwrap_or_else(|| revision.to_owned()),
            reason: report.stop_reason.map_or(
                Reason {
                    code: "removed",
                    explanation: "the tracked target was removed before this branch endpoint",
                },
                |reason| Reason { code: reason.code, explanation: reason.explanation },
            ),
        },
        FinalState::Unknown => Status::Unknown {
            location,
            stopped_at: report.stopped_at.unwrap_or_else(|| revision.to_owned()),
            reason: report.stop_reason.map_or(
                Reason {
                    code: "unknown_correspondence",
                    explanation: "the target correspondence could not be established on this branch",
                },
                |reason| Reason { code: reason.code, explanation: reason.explanation },
            ),
        },
    };
    BranchState { status, heads }
}

struct MergeEdge<'a> {
    repository: &'a Repository,
    parent: &'a str,
    merge: &'a str,
    changes: &'a [Change],
    hunks: &'a [Hunk],
}

fn merge_parent_state(
    repository: &Repository,
    parent: &str,
    merge: &str,
    state: &BranchState,
    selection: Option<&SymbolSelection>,
) -> Status {
    match &state.status {
        Status::Deleted {
            location,
            stopped_at,
            reason,
        } => Status::Deleted {
            location: location.clone(),
            stopped_at: stopped_at.clone(),
            reason: reason.clone(),
        },
        Status::Unknown {
            location,
            stopped_at,
            reason,
        } => Status::Unknown {
            location: location.clone(),
            stopped_at: stopped_at.clone(),
            reason: reason.clone(),
        },
        Status::Active { location, span } => {
            let Some((changes, hunks)) = repository.fate_parent_diff(parent, merge) else {
                return Status::unknown(
                    Some(location.clone()),
                    merge.to_owned(),
                    "merge_material_unavailable",
                    "complete local parent-to-merge material is unavailable or over budget",
                );
            };
            let Some(change) = changes.iter().find(|change| {
                !change.status.starts_with('C') && change.old_path.as_ref() == Some(&location.path)
            }) else {
                return Status::Active {
                    location: location.clone(),
                    span: *span,
                };
            };
            if change.status.starts_with('T') {
                return Status::unknown(
                    Some(location.clone()),
                    merge.to_owned(),
                    "non_textual_change",
                    "the tracked file changed type in the merge result",
                );
            }
            let edge = MergeEdge {
                repository,
                parent,
                merge,
                changes: &changes,
                hunks: &hunks,
            };
            if let Some(selection) = selection {
                merge_symbol(&edge, location, *span, selection)
            } else {
                merge_line(&edge, location)
            }
        }
    }
}

fn merge_line(edge: &MergeEdge<'_>, before: &Location) -> Status {
    let repository = edge.repository;
    let parent = edge.parent;
    let merge = edge.merge;
    let changes = edge.changes;
    let hunks = edge.hunks;
    let Some(change) = changes.iter().find(|change| {
        !change.status.starts_with('C') && change.old_path.as_ref() == Some(&before.path)
    }) else {
        return Status::Active {
            location: before.clone(),
            span: None,
        };
    };
    let mapped = if change.status.starts_with('D') {
        Some(super::Forward::Deleted)
    } else if let Some(new_path) = change.new_path.as_ref() {
        if change.old_blob == change.new_blob {
            return Status::Active {
                location: Location {
                    path: new_path.clone(),
                    line: before.line,
                },
                span: None,
            };
        }
        let history = history_hunks(hunks, change);
        if history.is_empty() {
            return Status::unknown(
                Some(before.clone()),
                merge.to_owned(),
                "missing_patch_material",
                "the merge edge changed the tracked file without usable line hunks",
            );
        }
        let Some(mapped) = super::map_line_forward(&history, before.line) else {
            return Status::unknown(
                Some(before.clone()),
                merge.to_owned(),
                "invalid_patch_mapping",
                "the merge-edge hunk ranges could not map the tracked line reliably",
            );
        };
        if let super::Forward::Kept { new_line } = mapped {
            return Status::Active {
                location: Location {
                    path: new_path.clone(),
                    line: new_line,
                },
                span: None,
            };
        }
        Some(mapped)
    } else {
        return Status::unknown(
            Some(before.clone()),
            merge.to_owned(),
            "merge_material_unavailable",
            "the merge diff did not identify a result path",
        );
    };
    let Some(old_content) = repository.read_blob(parent, &before.path).ok().flatten() else {
        return Status::unknown(
            Some(before.clone()),
            merge.to_owned(),
            "merge_material_unavailable",
            "the parent file source is unavailable for move verification",
        );
    };
    let context = super::line_move_context(&old_content, before.line, &before.path);
    let candidates = if let Some(context) = context {
        if !merge_candidate_sources_available(repository, merge, changes) {
            return Status::unknown(
                Some(before.clone()),
                merge.to_owned(),
                "merge_material_unavailable",
                "one or more merge result files needed for move verification are unavailable",
            );
        }
        match merge_line_move_candidates(repository, merge, changes, hunks, &context) {
            Ok(candidates) => Some(candidates),
            Err(_) => {
                return Status::unknown(
                    Some(before.clone()),
                    merge.to_owned(),
                    "merge_material_unavailable",
                    "line move candidates could not be verified from complete merge material",
                );
            }
        }
    } else {
        None
    };
    if let Some(candidates) = candidates {
        let total = candidates.added.len() + candidates.unverified.len();
        if candidates.added.len() == 1 && candidates.unverified.is_empty() {
            let (path, line) = candidates.added.into_iter().next().unwrap();
            return Status::Active {
                location: Location { path, line },
                span: None,
            };
        }
        if total > 0 {
            return Status::unknown(
                Some(before.clone()),
                merge.to_owned(),
                if candidates.unverified.is_empty() {
                    "ambiguous_merge_move"
                } else {
                    "merge_move_unverified"
                },
                "the merge result has no unique verified line continuation",
            );
        }
    }
    match mapped {
        Some(super::Forward::Deleted) => Status::Deleted {
            location: Some(before.clone()),
            stopped_at: merge.to_owned(),
            reason: Reason {
                code: "removed",
                explanation: "the tracked line was deleted in the merge result",
            },
        },
        Some(super::Forward::Rewritten { .. }) => Status::unknown(
            Some(before.clone()),
            merge.to_owned(),
            "line_rewritten",
            "the merge result rewrote the tracked line; continuation is not established",
        ),
        Some(super::Forward::Kept { .. }) | None => Status::unknown(
            Some(before.clone()),
            merge.to_owned(),
            "invalid_patch_mapping",
            "the merge-edge hunk ranges could not map the tracked line reliably",
        ),
    }
}

fn merge_symbol(
    edge: &MergeEdge<'_>,
    before: &Location,
    span: Option<(i64, i64)>,
    selection: &SymbolSelection,
) -> Status {
    let repository = edge.repository;
    let parent = edge.parent;
    let merge = edge.merge;
    let changes = edge.changes;
    let hunks = edge.hunks;
    let Some(span) = span else {
        return Status::unknown(
            Some(before.clone()),
            merge.to_owned(),
            "symbol_source_unavailable",
            "the symbol span was unavailable before the merge",
        );
    };
    let Some(old_content) = repository.read_blob(parent, &before.path).ok().flatten() else {
        return Status::unknown(
            Some(before.clone()),
            merge.to_owned(),
            "symbol_source_unavailable",
            "the parent symbol source is unavailable",
        );
    };
    let Ok(old_path_text) = std::str::from_utf8(&before.path) else {
        return Status::unknown(
            Some(before.clone()),
            merge.to_owned(),
            "symbol_source_unavailable",
            "the parent symbol path is not UTF-8",
        );
    };
    let Ok(old_symbol) =
        repository.locate_symbol(&old_content, &selection.qualified_name, old_path_text)
    else {
        return Status::unknown(
            Some(before.clone()),
            merge.to_owned(),
            "symbol_source_unavailable",
            "the parent symbol cannot be resolved uniquely",
        );
    };
    let change = changes.iter().find(|change| {
        !change.status.starts_with('C') && change.old_path.as_ref() == Some(&before.path)
    });
    let mut symbol_identity_resolved = false;
    let mut symbol_mapping_available = false;
    let mut symbol_lookup_reason = "symbol_declaration_replaced";
    let new_symbol = change.and_then(|change| {
        let Some(path) = change.new_path.as_ref() else {
            symbol_lookup_reason = "merge_material_unavailable";
            return None;
        };
        let Ok(path_text) = std::str::from_utf8(path) else {
            symbol_lookup_reason = "symbol_source_unavailable";
            return None;
        };
        let Some(content) = repository.read_blob(merge, path).ok().flatten() else {
            symbol_lookup_reason = "symbol_source_unavailable";
            return None;
        };
        let Ok(symbol) = repository.locate_symbol(&content, &selection.qualified_name, path_text)
        else {
            symbol_lookup_reason = "symbol_declaration_replaced";
            return None;
        };
        Some((path, content, symbol))
    });
    if let Some((path, new_content, new_symbol)) = new_symbol
        && new_symbol.selection.qualified_name == selection.qualified_name
        && new_symbol.selection.language == selection.language
    {
        symbol_identity_resolved = true;
        let end = i64::try_from(new_symbol.end_line).unwrap_or(i64::MAX);
        let new_location = Location {
            path: path.to_vec(),
            line: i64::try_from(new_symbol.start_line).unwrap_or(i64::MAX),
        };
        let new_start = new_location.line;
        let mapped = super::spans_map_forward_with_hunks(
            &change
                .map(|change| history_hunks(hunks, change))
                .unwrap_or_default(),
            super::SymbolSource {
                span,
                body_span: old_symbol.body_span,
                content: &old_content,
                language: &selection.language,
            },
            super::SymbolSource {
                span: (new_start, end),
                body_span: new_symbol.body_span,
                content: &new_content,
                language: &selection.language,
            },
        );
        symbol_mapping_available = mapped.is_some();
        if mapped == Some(true) {
            return Status::Active {
                location: new_location,
                span: Some((new_start, end)),
            };
        }
    }
    let Some(strict_target) = super::symbol_source_bytes(&old_content, span) else {
        return Status::unknown(
            Some(before.clone()),
            merge.to_owned(),
            "symbol_source_unavailable",
            "the parent symbol source bytes are unavailable",
        );
    };
    let search = super::SymbolMoveSearch {
        excluded_path: None,
        symbol_name: &selection.qualified_name,
        strict_target: &strict_target,
        language: &selection.language,
    };
    if !merge_candidate_sources_available(repository, merge, changes) {
        return Status::unknown(
            Some(before.clone()),
            merge.to_owned(),
            "merge_material_unavailable",
            "one or more merge result files needed for symbol verification are unavailable",
        );
    }
    let candidates = match merge_symbol_move_candidates(repository, merge, changes, hunks, search) {
        Ok(candidates) => candidates,
        Err(_) => {
            return Status::unknown(
                Some(before.clone()),
                merge.to_owned(),
                "merge_material_unavailable",
                "symbol move candidates could not be verified from complete merge material",
            );
        }
    };
    if candidates.added.len() == 1 && candidates.unverified.is_empty() {
        let candidate = candidates.added.into_iter().next().unwrap();
        return Status::Active {
            location: Location {
                path: candidate.path,
                line: candidate.start,
            },
            span: Some((candidate.start, candidate.end)),
        };
    }
    if !candidates.added.is_empty() || !candidates.unverified.is_empty() {
        return Status::unknown(
            Some(before.clone()),
            merge.to_owned(),
            if candidates.unverified.is_empty() {
                "ambiguous_merge_symbol_move"
            } else {
                "merge_symbol_move_unverified"
            },
            "the merge result has no unique verified symbol continuation",
        );
    }
    if change.is_some_and(|change| change.status.starts_with('D')) {
        Status::Deleted {
            location: Some(before.clone()),
            stopped_at: merge.to_owned(),
            reason: Reason {
                code: "removed",
                explanation: "the selected symbol was removed in the merge result",
            },
        }
    } else if !symbol_identity_resolved {
        Status::unknown(
            Some(before.clone()),
            merge.to_owned(),
            symbol_lookup_reason,
            "the selected symbol no longer resolves uniquely in the merge result",
        )
    } else if !symbol_mapping_available {
        Status::unknown(
            Some(before.clone()),
            merge.to_owned(),
            "merge_material_unavailable",
            "complete symbol correspondence material is unavailable for this merge edge",
        )
    } else {
        Status::unknown(
            Some(before.clone()),
            merge.to_owned(),
            "symbol_body_rewritten",
            "the merge result does not preserve enough symbol body code to establish continuation",
        )
    }
}

fn merge_line_move_candidates(
    repository: &Repository,
    merge: &str,
    changes: &[Change],
    hunks: &[Hunk],
    context: &super::LineMoveContext,
) -> Result<super::LineMoveCandidates, AppError> {
    let path_changes = path_changes(changes);
    super::find_line_move_candidates_with_added_lines(
        repository,
        merge,
        &path_changes,
        context,
        |ordinal, targets| {
            let Some(change) = changes.iter().find(|change| change.ordinal == ordinal) else {
                return Ok(None);
            };
            let hunks = history_hunks(hunks, change);
            Ok(super::added_target_lines(&hunks, false, false, targets))
        },
    )
}

fn merge_symbol_move_candidates(
    repository: &Repository,
    merge: &str,
    changes: &[Change],
    hunks: &[Hunk],
    search: super::SymbolMoveSearch<'_>,
) -> Result<super::SymbolMoveCandidates, AppError> {
    let path_changes = path_changes(changes);
    super::find_symbol_move_candidates_with_addition_status(
        repository,
        merge,
        &path_changes,
        search,
        |ordinal, span| {
            let Some(change) = changes.iter().find(|change| change.ordinal == ordinal) else {
                return Ok(super::AddedMaterialStatus::Unavailable);
            };
            let hunks = history_hunks(hunks, change);
            Ok(super::span_addition_status_from_hunks(&hunks, span))
        },
    )
}

fn merge_candidate_sources_available(
    repository: &Repository,
    revision: &str,
    changes: &[Change],
) -> bool {
    changes.iter().all(|change| {
        if change.status.starts_with('D') || change.status.starts_with('T') {
            return true;
        }
        change.new_path.as_deref().is_some_and(|path| {
            repository
                .read_blob(revision, path)
                .is_ok_and(|blob| blob.is_some())
        })
    })
}

fn path_changes(changes: &[Change]) -> Vec<crate::cache::PathChange> {
    changes
        .iter()
        .map(|change| crate::cache::PathChange {
            ordinal: change.ordinal,
            status: change.status.clone(),
            old_path: change.old_path.clone(),
            new_path: change.new_path.clone(),
            old_blob: change.old_blob.clone(),
            new_blob: change.new_blob.clone(),
        })
        .collect()
}

fn history_hunks(hunks: &[Hunk], change: &Change) -> Vec<crate::cache::PatchHistoryHunk> {
    hunks
        .iter()
        .filter(|hunk| hunk.change_ordinal == change.ordinal)
        .map(|hunk| crate::cache::PatchHistoryHunk {
            change_ordinal: hunk.change_ordinal,
            old_path: change.old_path.clone(),
            new_path: change.new_path.clone(),
            old_start: hunk.old_start,
            old_lines: hunk.old_lines,
            new_start: hunk.new_start,
            new_lines: hunk.new_lines,
            hunk_ordinal: hunk.ordinal,
            text: Some(hunk.text.clone()),
        })
        .collect()
}

fn reconcile(outcomes: &[Status], merge: &str) -> (Status, Option<Location>, Option<(i64, i64)>) {
    let unknown = outcomes
        .iter()
        .find(|status| matches!(status, Status::Unknown { stopped_at, .. } if stopped_at != merge))
        .or_else(|| {
            outcomes
                .iter()
                .find(|status| matches!(status, Status::Unknown { .. }))
        });
    if let Some(Status::Unknown {
        location,
        stopped_at,
        reason,
    }) = unknown
    {
        let status = Status::unknown(
            location.clone(),
            stopped_at.clone(),
            reason.code,
            reason.explanation,
        );
        return (status, None, None);
    }
    let active = outcomes
        .iter()
        .filter_map(|status| match status {
            Status::Active { location, span } => Some((location.clone(), *span)),
            _ => None,
        })
        .collect::<Vec<_>>();
    if let Some((location, span)) = active.first().cloned() {
        if active
            .iter()
            .any(|(candidate, candidate_span)| *candidate != location || *candidate_span != span)
        {
            return (
                Status::unknown(
                    Some(location),
                    merge.to_owned(),
                    "merge_lineage_ambiguous",
                    "relevant parents map the target to different result locations",
                ),
                None,
                None,
            );
        }
        return (
            Status::Active {
                location: location.clone(),
                span,
            },
            Some(location),
            span,
        );
    }
    let location = outcomes.iter().find_map(|status| match status {
        Status::Deleted { location, .. } => location.clone(),
        _ => None,
    });
    (
        Status::Deleted {
            location,
            stopped_at: merge.to_owned(),
            reason: Reason {
                code: "removed",
                explanation: "all relevant merge parents had deleted the target",
            },
        },
        None,
        None,
    )
}

fn status_stop(status: &Status) -> (Option<String>, Option<Reason>) {
    match status {
        Status::Active { .. } => (None, None),
        Status::Deleted {
            stopped_at, reason, ..
        }
        | Status::Unknown {
            stopped_at, reason, ..
        } => (Some(stopped_at.clone()), Some(reason.clone())),
    }
}

fn push_warning(warnings: &mut Vec<String>, warning: &str) {
    if !warnings.iter().any(|existing| existing == warning) {
        warnings.push(warning.to_owned());
    }
}

impl Status {
    fn unknown(
        location: Option<Location>,
        stopped_at: String,
        code: &'static str,
        explanation: &'static str,
    ) -> Self {
        Self::Unknown {
            location,
            stopped_at,
            reason: Reason { code, explanation },
        }
    }
}
