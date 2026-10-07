//! Forward fate tracking for one historical line through linear history.
//!
//! The tracked line is validated at the starting revision, then mapped forward
//! through cached endpoint-reachable history. Unchanged lines follow preceding
//! insertions and deletions and detected renames without becoming events; a
//! rewritten line is one bounded replacement event and then tracking stops.
//! Unsupported or missing correspondence stops explicitly as unknown.

use std::collections::HashSet;

use crate::cache::{PatchHistoryHunk, QuerySession};
use crate::git::Repository;

use super::normalize_git_path_string;
use crate::analysis::query::{Context, Options, Outcome, QueryReport, scope};
use crate::analysis::{PatchExcerpt, SearchScopeInfo};
use crate::git::SymbolSelection;
use crate::{app::AppError, cache::followups::ForwardCommit};

const MAX_HUNKS: usize = 4_096;
const MAX_HUNK_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FinalState {
    ReachedEndpoint,
    Deleted,
    Unknown,
}

impl FinalState {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::ReachedEndpoint => "reached_endpoint",
            Self::Deleted => "deleted",
            Self::Unknown => "unknown",
        }
    }
}

pub(crate) struct StopReason {
    pub(crate) code: &'static str,
    pub(crate) explanation: &'static str,
}

#[derive(Clone)]
pub(crate) struct Location {
    pub(crate) path: Vec<u8>,
    pub(crate) line: i64,
}

pub(crate) struct Event {
    pub(crate) commit_id: String,
    pub(crate) subject: String,
    pub(crate) commit_time: i64,
    pub(crate) relationship: &'static str,
    pub(crate) before: Location,
    pub(crate) after: Option<Location>,
    /// Inclusive symbol span at `before`; `None` for line targets.
    pub(crate) before_span: Option<(i64, i64)>,
    /// Inclusive symbol span at `after`; `None` for line targets and removals.
    pub(crate) after_span: Option<(i64, i64)>,
    pub(crate) parent_count: usize,
    pub(crate) patch: Option<PatchExcerpt>,
}

pub(crate) struct Report {
    pub(crate) start_revision: String,
    pub(crate) endpoint: String,
    pub(crate) path: Vec<u8>,
    pub(crate) line: usize,
    /// Structured selection when the target was chosen by `--symbol`.
    pub(crate) symbol_selection: Option<SymbolSelection>,
    pub(crate) final_state: FinalState,
    pub(crate) stopped_at: Option<String>,
    pub(crate) stop_reason: Option<StopReason>,
    pub(crate) last_location: Option<Location>,
    /// Inclusive line span at `last_location` when the target is a symbol.
    pub(crate) last_span: Option<(i64, i64)>,
    pub(crate) inspected_commits: usize,
    pub(crate) max_commits: Option<usize>,
    pub(crate) traversal_truncated: bool,
    pub(crate) total_events: usize,
    pub(crate) display_truncated: bool,
    pub(crate) patch_mode: bool,
    pub(crate) limit: usize,
    pub(crate) events: Vec<Event>,
    pub(crate) scope: Option<SearchScopeInfo>,
}

enum Forward {
    Kept { new_line: i64 },
    Deleted,
    Rewritten { replacement: Option<i64> },
}

pub(in crate::analysis) fn execute(
    path: String,
    line: Option<usize>,
    symbol: Option<String>,
    at: String,
    to_rev: Option<String>,
    max_commits: Option<usize>,
    options: Options,
) -> Result<Outcome, AppError> {
    let path = normalize_git_path_string(&path);
    let repository = Repository::discover()?;
    let (context, target) =
        Context::prepare_target(&repository, Some(&at), options.scope.clone(), |revision| {
            repository.pin_fate_target(revision, &path, line, symbol.as_deref())
        })?;
    let session = &context.session;

    if target.symbol_selection.is_some() {
        return execute_symbol(
            session,
            &repository,
            &context,
            target,
            to_rev,
            max_commits,
            options,
        );
    }
    execute_line(
        session,
        &repository,
        &context,
        target,
        to_rev,
        max_commits,
        options,
    )
}

fn execute_line(
    session: &crate::cache::QuerySession,
    repository: &Repository,
    context: &Context,
    target: crate::git::FateTarget,
    to_rev: Option<String>,
    max_commits: Option<usize>,
    options: Options,
) -> Result<Outcome, AppError> {
    let (endpoint, traversal_endpoint, endpoint_frontier) = match to_rev {
        Some(revision) => {
            let endpoint = repository.resolve_commit(&revision)?;
            session.require_revision(&endpoint)?;
            (endpoint.clone(), endpoint, false)
        }
        None => {
            let head = context.pinned_head.clone();
            if session.contains_revision(&head)? {
                (head.clone(), head, false)
            } else {
                let history = scope::reachable_history(session, repository, &head)?;
                let frontier = history.revisions.first().cloned().ok_or_else(|| {
                    AppError::input(
                        "no commits reachable from current HEAD are available after query refresh; run `gitscry index` to publish reachable history",
                    )
                })?;
                (head, frontier, true)
            }
        }
    };

    let mut warnings = target.warnings.clone();
    let coverage = scope::reachable_history(session, repository, &traversal_endpoint)?;
    if endpoint_frontier || !coverage.coverage_complete {
        warnings.push(
            "Endpoint-reachable local history is incomplete; fate tracking covers only commits available in the published cache. Run `gitscry index` after making additional history available."
                .into(),
        );
    }

    if !repository.is_ancestor(&target.revision, &endpoint)? {
        return Err(AppError::input(
            "fate starting revision must be an ancestor of the endpoint",
        ));
    }

    if target.revision == endpoint {
        let path = target.path.clone();
        return Ok(finished(
            session,
            Report {
                start_revision: target.revision,
                endpoint,
                path,
                line: target.line,
                symbol_selection: None,
                final_state: FinalState::ReachedEndpoint,
                stopped_at: None,
                stop_reason: None,
                last_location: Some(Location {
                    path: target.path,
                    line: i64::from(u32::try_from(target.line).unwrap_or(0)),
                }),
                last_span: None,
                inspected_commits: 0,
                max_commits,
                traversal_truncated: false,
                total_events: 0,
                display_truncated: false,
                patch_mode: options.patch,
                limit: options.limit,
                events: Vec::new(),
                scope: None,
            },
            warnings,
        ));
    }

    let graph = session.forward_graph(&traversal_endpoint)?;
    let tracked = descendants(&graph, &target.revision);

    let mut state = Location {
        path: target.path.clone(),
        line: i64::try_from(target.line)
            .map_err(|_| AppError::input("line does not fit the tracked coordinate space"))?,
    };
    let mut last_revision = target.revision.clone();
    let mut events: Vec<Event> = Vec::new();
    let mut inspected = 0usize;
    let ceiling = max_commits.unwrap_or(usize::MAX);
    let mut traversal_truncated = false;
    let mut stop: Option<(String, FinalState, StopReason)> = None;

    for node in graph.iter().filter(|node| tracked.contains(&node.oid)) {
        if inspected == ceiling {
            traversal_truncated = true;
            warnings.push(
                "Inspection stopped at the --max-commits budget; uninspected forward commits are not accounted for."
                    .into(),
            );
            stop = Some((
                last_revision.clone(),
                FinalState::Unknown,
                StopReason {
                    code: "max_commits_exhausted",
                    explanation: "the --max-commits inspection budget was exhausted before the endpoint",
                },
            ));
            break;
        }
        inspected += 1;
        if node.parents.len() > 1 {
            stop = Some((
                node.oid.clone(),
                FinalState::Unknown,
                StopReason {
                    code: "merge_history",
                    explanation: "merge commits are not traversed by this slice",
                },
            ));
            break;
        }
        if node.parents.first() != Some(&last_revision) {
            stop = Some((
                node.oid.clone(),
                FinalState::Unknown,
                StopReason {
                    code: "discontinuous_correspondence",
                    explanation: "the previous commit does not directly precede this commit; non-linear history is not traversed by this slice",
                },
            ));
            break;
        }
        let changes = session.forward_changes(&node.oid)?;
        let Some(change) = changes.iter().find(|change| {
            !change.status.starts_with('C') && change.old_path.as_ref() == Some(&state.path)
        }) else {
            last_revision = node.oid.clone();
            continue;
        };
        if change.status.starts_with('D') {
            events.push(Event {
                commit_id: node.oid.clone(),
                subject: session.forward_subject(&node.oid)?,
                commit_time: node.commit_time,
                relationship: "removed",
                before: state.clone(),
                before_span: None,
                after_span: None,
                after: None,
                parent_count: node.parents.len(),
                patch: event_patch(
                    session,
                    options.patch,
                    &node.oid,
                    change.ordinal,
                    state.line,
                )?,
            });
            stop = Some((
                node.oid.clone(),
                FinalState::Deleted,
                StopReason {
                    code: "removed",
                    explanation: "the tracked file was removed with no observed continuation",
                },
            ));
            break;
        }
        if change.status.starts_with('T') {
            stop = Some((
                node.oid.clone(),
                FinalState::Unknown,
                StopReason {
                    code: "non_textual_change",
                    explanation: "the tracked file changed type; textual line correspondence is unavailable",
                },
            ));
            break;
        }
        let new_path = change
            .new_path
            .clone()
            .unwrap_or_else(|| state.path.clone());
        let renamed = change
            .old_path
            .as_ref()
            .is_some_and(|old_path| old_path != &new_path);
        if change.old_blob == change.new_blob {
            if renamed {
                events.push(Event {
                    commit_id: node.oid.clone(),
                    subject: session.forward_subject(&node.oid)?,
                    commit_time: node.commit_time,
                    relationship: "rename",
                    before: state.clone(),
                    before_span: None,
                    after_span: None,
                    after: Some(Location {
                        path: new_path.clone(),
                        line: state.line,
                    }),
                    parent_count: node.parents.len(),
                    patch: event_patch(
                        session,
                        options.patch,
                        &node.oid,
                        change.ordinal,
                        state.line,
                    )?,
                });
            }
            state.path = new_path;
            last_revision = node.oid.clone();
            continue;
        }
        let history = session.patch_history_for_change(
            &node.oid,
            change.ordinal,
            MAX_HUNKS,
            MAX_HUNK_BYTES,
        )?;
        if history.missing_objects
            || history.truncated
            || history.hunks.is_empty()
            || history.hunks.iter().any(|hunk| hunk.text.is_none())
        {
            stop = Some((
                node.oid.clone(),
                FinalState::Unknown,
                StopReason {
                    code: "missing_patch_material",
                    explanation: "cached patch material needed to map the tracked line is missing or truncated",
                },
            ));
            break;
        }
        match map_line_forward(&history.hunks, state.line) {
            Some(Forward::Kept { new_line }) => {
                if renamed {
                    events.push(Event {
                        commit_id: node.oid.clone(),
                        subject: session.forward_subject(&node.oid)?,
                        commit_time: node.commit_time,
                        relationship: "rename",
                        before: state.clone(),
                        before_span: None,
                        after_span: None,
                        after: Some(Location {
                            path: new_path.clone(),
                            line: new_line,
                        }),
                        parent_count: node.parents.len(),
                        patch: event_patch(
                            session,
                            options.patch,
                            &node.oid,
                            change.ordinal,
                            state.line,
                        )?,
                    });
                }
                state.path = new_path;
                state.line = new_line;
            }
            Some(Forward::Rewritten { replacement }) => {
                events.push(Event {
                    commit_id: node.oid.clone(),
                    subject: session.forward_subject(&node.oid)?,
                    commit_time: node.commit_time,
                    relationship: "rewrite",
                    before: state.clone(),
                    before_span: None,
                    after_span: None,
                    after: replacement.map(|line| Location {
                        path: new_path,
                        line,
                    }),
                    parent_count: node.parents.len(),
                    patch: event_patch(
                        session,
                        options.patch,
                        &node.oid,
                        change.ordinal,
                        state.line,
                    )?,
                });
                stop = Some((
                    node.oid.clone(),
                    FinalState::Unknown,
                    StopReason {
                        code: "line_rewritten",
                        explanation: "the replacement is not established as the same historical line",
                    },
                ));
            }
            Some(Forward::Deleted) => {
                events.push(Event {
                    commit_id: node.oid.clone(),
                    subject: session.forward_subject(&node.oid)?,
                    commit_time: node.commit_time,
                    relationship: "removed",
                    before: state.clone(),
                    before_span: None,
                    after_span: None,
                    after: None,
                    parent_count: node.parents.len(),
                    patch: event_patch(
                        session,
                        options.patch,
                        &node.oid,
                        change.ordinal,
                        state.line,
                    )?,
                });
                stop = Some((
                    node.oid.clone(),
                    FinalState::Deleted,
                    StopReason {
                        code: "removed",
                        explanation: "the tracked line was deleted",
                    },
                ));
            }
            None => {
                stop = Some((
                    node.oid.clone(),
                    FinalState::Unknown,
                    StopReason {
                        code: "invalid_patch_mapping",
                        explanation: "cached patch hunk ranges could not be mapped reliably",
                    },
                ));
            }
        }
        if stop.is_some() {
            break;
        }
        last_revision = node.oid.clone();
    }

    let (final_state, stopped_at, stop_reason, last_location) = match stop {
        Some((oid, stop_state, reason)) => {
            let last = if matches!(stop_state, FinalState::Deleted) {
                events
                    .last()
                    .map(|event| event.after.clone().unwrap_or_else(|| event.before.clone()))
            } else {
                Some(state.clone())
            };
            (stop_state, Some(oid), Some(reason), last)
        }
        None if endpoint_frontier => (
            FinalState::Unknown,
            Some(endpoint.clone()),
            Some(StopReason {
                code: "endpoint_not_covered",
                explanation: "cached coverage ends before the requested endpoint; survival to the endpoint is not established",
            }),
            Some(state.clone()),
        ),
        None => (FinalState::ReachedEndpoint, None, None, Some(state)),
    };

    let total_events = events.len();
    let display_truncated = total_events > options.limit;
    let displayed = events
        .into_iter()
        .rev()
        .take(options.limit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    Ok(finished(
        session,
        Report {
            start_revision: target.revision,
            endpoint,
            path: target.path,
            line: target.line,
            symbol_selection: None,
            final_state,
            stopped_at,
            stop_reason,
            last_location,
            last_span: None,
            inspected_commits: inspected,
            max_commits,
            traversal_truncated,
            total_events,
            display_truncated,
            patch_mode: options.patch,
            limit: options.limit,
            events: displayed,
            scope: None,
        },
        warnings,
    ))
}

fn finished(session: &QuerySession, report: Report, warnings: Vec<String>) -> Outcome {
    let mut all_warnings = session.warnings().to_vec();
    all_warnings.extend(warnings);
    Outcome {
        progress: session.progress().to_vec(),
        warnings: all_warnings,
        report: QueryReport::Fate(report),
    }
}

fn event_patch(
    session: &QuerySession,
    include_patch: bool,
    oid: &str,
    change_ordinal: i64,
    line: i64,
) -> Result<Option<PatchExcerpt>, AppError> {
    if !include_patch {
        return Ok(None);
    }
    let patch = crate::analysis::patch::selected_patch_excerpt(session, oid, |hunk| {
        if hunk.change_ordinal != change_ordinal || hunk.old_lines <= 0 {
            return None;
        }
        let end = hunk.old_start.checked_add(hunk.old_lines)?;
        (line >= hunk.old_start && line < end).then_some(0)
    })?;
    Ok(Some(patch))
}

/// Cache positions are the published reverse-topological Git order, not timestamps.
/// Mark descendants using all parents, excluding the tracked starting revision.
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

/// Map a tracked old-file line forward through one change's cached hunks.
/// Returns `None` when the cached hunk ranges cannot be mapped reliably.
fn map_line_forward(hunks: &[PatchHistoryHunk], line: i64) -> Option<Forward> {
    let mut ordered: Vec<&PatchHistoryHunk> = hunks.iter().collect();
    ordered.sort_by_key(|hunk| hunk.old_start);
    let mut new_line = line;
    for hunk in ordered {
        if line < hunk.old_start {
            break;
        }
        if line >= hunk.old_start + hunk.old_lines {
            new_line += hunk.new_lines - hunk.old_lines;
            continue;
        }
        let text = hunk.text.as_ref()?;
        let mut old = hunk.old_start;
        let mut new = hunk.new_start;
        let mut old_count = 0i64;
        let mut new_count = 0i64;
        let mut deleted_total = 0i64;
        let mut added_total = 0i64;
        let mut added_position: Option<i64> = None;
        let mut target_deleted = false;
        let mut target_new_line = None;
        for diff_line in text.split_inclusive(|byte| *byte == b'\n') {
            match diff_line.first() {
                Some(b'-') => {
                    deleted_total += 1;
                    if old == line {
                        target_deleted = true;
                    }
                    old += 1;
                    old_count += 1;
                }
                Some(b'+') => {
                    added_total += 1;
                    if added_position.is_none() {
                        added_position = Some(new);
                    }
                    new += 1;
                    new_count += 1;
                }
                Some(b' ') => {
                    if old == line {
                        target_new_line = Some(new);
                    }
                    old += 1;
                    new += 1;
                    old_count += 1;
                    new_count += 1;
                }
                Some(b'@' | b'\\') => {}
                _ => return None,
            }
        }
        if old_count != hunk.old_lines || new_count != hunk.new_lines {
            return None;
        }
        if target_deleted {
            if added_total == 0 {
                return Some(Forward::Deleted);
            }
            let replacement = (deleted_total == 1 && added_total == 1)
                .then_some(())
                .and(added_position);
            return Some(Forward::Rewritten { replacement });
        }
        return Some(Forward::Kept {
            new_line: target_new_line?,
        });
    }
    Some(Forward::Kept { new_line })
}

/// Track a historical symbol declaration forward through linear history.
///
/// The declaration is resolved at the starting revision; every forward commit
/// must re-resolve it uniquely in the new source version. Continuity uses the
/// normalized code body (comments and whitespace excluded), never the name
/// alone: local edits that preserve body code shape are modification events,
/// while whole-body rewrites, declaration replacement, and unresolvable moves
/// stop explicitly as unknown.
fn execute_symbol(
    session: &crate::cache::QuerySession,
    repository: &Repository,
    context: &Context,
    target: crate::git::FateTarget,
    to_rev: Option<String>,
    max_commits: Option<usize>,
    options: Options,
) -> Result<Outcome, AppError> {
    let mut warnings = target.warnings.clone();
    let (endpoint, traversal_endpoint, endpoint_frontier) = resolve_endpoint(
        session,
        repository,
        context,
        to_rev.as_deref(),
        &mut warnings,
    )?;

    if !repository.is_ancestor(&target.revision, &endpoint)? {
        return Err(AppError::input(
            "fate starting revision must be an ancestor of the endpoint",
        ));
    }

    let selection = target
        .symbol_selection
        .clone()
        .expect("symbol target has a selection");
    let qualified_name = selection.qualified_name.clone();

    if target.revision == endpoint {
        return Ok(finished(
            session,
            Report {
                start_revision: target.revision,
                endpoint,
                path: target.path.clone(),
                line: target.line,
                symbol_selection: Some(selection),
                final_state: FinalState::ReachedEndpoint,
                stopped_at: None,
                stop_reason: None,
                last_location: Some(Location {
                    path: target.path,
                    line: i64::try_from(target.line)
                        .map_err(|_| AppError::input("line does not fit the coordinate space"))?,
                }),
                last_span: Some((
                    i64::try_from(target.line)
                        .map_err(|_| AppError::input("line does not fit the coordinate space"))?,
                    i64::try_from(target.symbol_end_line.unwrap_or(target.line))
                        .map_err(|_| AppError::input("line does not fit the coordinate space"))?,
                )),
                inspected_commits: 0,
                max_commits,
                traversal_truncated: false,
                total_events: 0,
                display_truncated: false,
                patch_mode: options.patch,
                limit: options.limit,
                events: Vec::new(),
                scope: None,
            },
            warnings,
        ));
    }

    let graph = session.forward_graph(&traversal_endpoint)?;
    let tracked = descendants(&graph, &target.revision);

    let mut state_path = target.path.clone();
    let mut state_start = i64::try_from(target.line)
        .map_err(|_| AppError::input("line does not fit the coordinate space"))?;
    let mut state_end = i64::try_from(target.symbol_end_line.unwrap_or(target.line))
        .map_err(|_| AppError::input("line does not fit the coordinate space"))?;
    let mut last_revision = target.revision.clone();
    let mut events: Vec<Event> = Vec::new();
    let mut inspected = 0usize;
    let ceiling = max_commits.unwrap_or(usize::MAX);
    let mut traversal_truncated = false;
    let mut stop: Option<(String, FinalState, StopReason)> = None;

    for node in graph.iter().filter(|node| tracked.contains(&node.oid)) {
        if inspected == ceiling {
            traversal_truncated = true;
            warnings.push(
                "Inspection stopped at the --max-commits budget; uninspected forward commits are not accounted for."
                    .into(),
            );
            stop = Some((
                last_revision.clone(),
                FinalState::Unknown,
                StopReason {
                    code: "max_commits_exhausted",
                    explanation: "the --max-commits inspection budget was exhausted before the endpoint",
                },
            ));
            break;
        }
        inspected += 1;
        if node.parents.len() > 1 {
            stop = Some((
                node.oid.clone(),
                FinalState::Unknown,
                StopReason {
                    code: "merge_history",
                    explanation: "merge commits are not traversed by this slice",
                },
            ));
            break;
        }
        if node.parents.first() != Some(&last_revision) {
            stop = Some((
                node.oid.clone(),
                FinalState::Unknown,
                StopReason {
                    code: "discontinuous_correspondence",
                    explanation: "the previous commit does not directly precede this commit; non-linear history is not traversed by this slice",
                },
            ));
            break;
        }
        let changes = session.forward_changes(&node.oid)?;
        let Some(change) = changes.iter().find(|change| {
            !change.status.starts_with('C') && change.old_path.as_ref() == Some(&state_path)
        }) else {
            last_revision = node.oid.clone();
            continue;
        };
        if change.status.starts_with('D') {
            push_symbol_event(
                &mut events,
                session,
                &node.oid,
                node.commit_time,
                "removed",
                node.parents.len(),
                &state_path,
                state_start,
                state_end,
                None,
                0,
                0,
                options.patch,
                change.ordinal,
            )?;
            stop = Some((
                node.oid.clone(),
                FinalState::Deleted,
                StopReason {
                    code: "removed",
                    explanation: "the tracked symbol's file was removed with no observed continuation",
                },
            ));
            break;
        }
        if change.status.starts_with('T') {
            stop = Some((
                node.oid.clone(),
                FinalState::Unknown,
                StopReason {
                    code: "non_textual_change",
                    explanation: "the tracked file changed type; textual symbol correspondence is unavailable",
                },
            ));
            break;
        }
        let new_path = change
            .new_path
            .clone()
            .unwrap_or_else(|| state_path.clone());
        let renamed = change
            .old_path
            .as_ref()
            .is_some_and(|old_path| old_path != &new_path);

        let Some(new_content) = repository.read_blob(&node.oid, &new_path)? else {
            stop = Some((
                node.oid.clone(),
                FinalState::Unknown,
                StopReason {
                    code: "symbol_source_unavailable",
                    explanation: "the tracked file's new contents are unavailable at this commit",
                },
            ));
            break;
        };
        let new_path_str = match std::str::from_utf8(&new_path) {
            Ok(path) => path,
            Err(_) => {
                stop = Some((
                    node.oid.clone(),
                    FinalState::Unknown,
                    StopReason {
                        code: "symbol_source_unavailable",
                        explanation: "the tracked file path is not UTF-8; symbol resolution is unavailable",
                    },
                ));
                break;
            }
        };
        let new_location = match repository.locate_symbol(
            &new_content,
            &qualified_name,
            new_path_str,
        ) {
            Ok(location) => location,
            Err(_) => {
                // The declaration no longer resolves uniquely in the new version:
                // replacement, rename, or removal of the declaration.
                if renamed {
                    push_symbol_event(
                        &mut events,
                        session,
                        &node.oid,
                        node.commit_time,
                        "rename",
                        node.parents.len(),
                        &state_path,
                        state_start,
                        state_end,
                        Some(&new_path),
                        state_start,
                        state_end,
                        options.patch,
                        change.ordinal,
                    )?;
                }
                stop = Some((
                    node.oid.clone(),
                    FinalState::Unknown,
                    StopReason {
                        code: "symbol_declaration_replaced",
                        explanation: "the declaration no longer resolves uniquely in the new version; renaming or replacement is not followed",
                    },
                ));
                break;
            }
        };
        if new_location.selection.qualified_name != qualified_name {
            stop = Some((
                node.oid.clone(),
                FinalState::Unknown,
                StopReason {
                    code: "symbol_declaration_replaced",
                    explanation: "the resolved declaration changed ownership in the new version; renaming is not followed",
                },
            ));
            break;
        }

        let new_start = i64::try_from(new_location.start_line)
            .map_err(|_| AppError::input("line does not fit the coordinate space"))?;
        let new_end = i64::try_from(new_location.end_line)
            .map_err(|_| AppError::input("line does not fit the coordinate space"))?;

        if renamed {
            push_symbol_event(
                &mut events,
                session,
                &node.oid,
                node.commit_time,
                "rename",
                node.parents.len(),
                &state_path,
                state_start,
                state_end,
                Some(&new_path),
                new_start,
                new_end,
                options.patch,
                change.ordinal,
            )?;
        }

        let new_identity = code_identity(&new_content, new_start, new_end);
        let old_content = repository.read_blob(&last_revision, &state_path)?;
        let old_identity = old_content
            .as_deref()
            .map(|content| code_identity(content, state_start, state_end));

        if old_identity.as_deref() != Some(new_identity.as_slice()) {
            // The normalized body changed: allow only a local edit whose old
            // span maps line-by-line onto the new span with at least one
            // unchanged body line as correspondence material.
            let local_edit = old_content.is_some_and(|content| {
                is_local_body_edit(
                    session,
                    &node.oid,
                    change.ordinal,
                    &content,
                    (state_start, state_end),
                    (new_start, new_end),
                )
            });
            if !local_edit {
                stop = Some((
                    node.oid.clone(),
                    FinalState::Unknown,
                    StopReason {
                        code: "symbol_body_rewritten",
                        explanation: "the declaration body was rewritten; unchanged body code no longer establishes continuity",
                    },
                ));
                break;
            }
            push_symbol_event(
                &mut events,
                session,
                &node.oid,
                node.commit_time,
                "modified",
                node.parents.len(),
                &state_path,
                state_start,
                state_end,
                Some(&new_path),
                new_start,
                new_end,
                options.patch,
                change.ordinal,
            )?;
        } else if !renamed && new_start != state_start {
            push_symbol_event(
                &mut events,
                session,
                &node.oid,
                node.commit_time,
                "shifted",
                node.parents.len(),
                &state_path,
                state_start,
                state_end,
                Some(&new_path),
                new_start,
                new_end,
                options.patch,
                change.ordinal,
            )?;
        }

        state_path = new_path;
        state_start = new_start;
        state_end = new_end;
        last_revision = node.oid.clone();
    }

    let (final_state, stopped_at, stop_reason, last_location, last_span) = match stop {
        Some((oid, stop_state, reason)) => {
            let last = if matches!(stop_state, FinalState::Deleted) {
                events.last().map(|event| Location {
                    path: event.after.as_ref().unwrap_or(&event.before).path.clone(),
                    line: event
                        .after_span
                        .unwrap_or(event.before_span.unwrap_or((0, 0)))
                        .0,
                })
            } else {
                Some(Location {
                    path: state_path,
                    line: state_start,
                })
            };
            let span = if matches!(stop_state, FinalState::Deleted) {
                events
                    .last()
                    .and_then(|event| event.after_span.or(event.before_span))
            } else {
                Some((state_start, state_end))
            };
            (stop_state, Some(oid), Some(reason), last, span)
        }
        None if endpoint_frontier => (
            FinalState::Unknown,
            Some(endpoint.clone()),
            Some(StopReason {
                code: "endpoint_not_covered",
                explanation: "cached coverage ends before the requested endpoint; survival to the endpoint is not established",
            }),
            Some(Location {
                path: state_path,
                line: state_start,
            }),
            Some((state_start, state_end)),
        ),
        None => (
            FinalState::ReachedEndpoint,
            None,
            None,
            Some(Location {
                path: state_path,
                line: state_start,
            }),
            Some((state_start, state_end)),
        ),
    };

    let total_events = events.len();
    let display_truncated = total_events > options.limit;
    let displayed = events
        .into_iter()
        .rev()
        .take(options.limit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    Ok(finished(
        session,
        Report {
            start_revision: target.revision,
            endpoint,
            path: target.path,
            line: target.line,
            symbol_selection: Some(selection),
            final_state,
            stopped_at,
            stop_reason,
            last_location,
            last_span,
            inspected_commits: inspected,
            max_commits,
            traversal_truncated,
            total_events,
            display_truncated,
            patch_mode: options.patch,
            limit: options.limit,
            events: displayed,
            scope: None,
        },
        warnings,
    ))
}

#[allow(clippy::too_many_arguments)]
fn resolve_endpoint(
    session: &crate::cache::QuerySession,
    repository: &Repository,
    context: &Context,
    to_rev: Option<&str>,
    warnings: &mut Vec<String>,
) -> Result<(String, String, bool), AppError> {
    match to_rev {
        Some(revision) => {
            let endpoint = repository.resolve_commit(revision)?;
            session.require_revision(&endpoint)?;
            Ok((endpoint.clone(), endpoint, false))
        }
        None => {
            let head = context.pinned_head.clone();
            if session.contains_revision(&head)? {
                Ok((head.clone(), head, false))
            } else {
                let history = scope::reachable_history(session, repository, &head)?;
                let frontier = history.revisions.first().cloned().ok_or_else(|| {
                    AppError::input(
                        "no commits reachable from current HEAD are available after query refresh; run `gitscry index` to publish reachable history",
                    )
                })?;
                warnings.push(
                    "Endpoint-reachable local history is incomplete; fate tracking covers only commits available in the published cache. Run `gitscry index` after making additional history available."
                        .into(),
                );
                Ok((head, frontier, true))
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_symbol_event(
    events: &mut Vec<Event>,
    session: &crate::cache::QuerySession,
    oid: &str,
    commit_time: i64,
    relationship: &'static str,
    parent_count: usize,
    before_path: &[u8],
    before_start: i64,
    before_end: i64,
    after_path: Option<&[u8]>,
    after_start: i64,
    after_end: i64,
    include_patch: bool,
    change_ordinal: i64,
) -> Result<(), AppError> {
    let patch = if include_patch {
        crate::analysis::patch::selected_patch_excerpt(session, oid, |hunk| {
            (hunk.change_ordinal == change_ordinal
                && hunk.old_start <= before_end
                && before_start < hunk.old_start + hunk.old_lines)
                .then_some(0)
        })
        .ok()
    } else {
        None
    };
    events.push(Event {
        commit_id: oid.to_owned(),
        subject: session.forward_subject(oid)?,
        commit_time,
        relationship,
        before: Location {
            path: before_path.to_vec(),
            line: before_start,
        },
        after: after_path.map(|path| Location {
            path: path.to_vec(),
            line: after_start,
        }),
        before_span: Some((before_start, before_end)),
        after_span: after_path.map(|_| (after_start, after_end)),
        parent_count,
        patch,
    });
    Ok(())
}

/// Decide whether the body change is a local edit: the old span's lines must
/// map one-to-one onto the new span through the cached hunks, with at least
/// one unchanged body line kept as correspondence material.
fn is_local_body_edit(
    session: &crate::cache::QuerySession,
    oid: &str,
    change_ordinal: i64,
    old_content: &[u8],
    old_span: (i64, i64),
    new_span: (i64, i64),
) -> bool {
    let (old_start, old_end) = old_span;
    let (new_start, new_end) = new_span;
    if new_end - new_start != old_end - old_start {
        return false;
    }
    let Ok(history) =
        session.patch_history_for_change(oid, change_ordinal, MAX_HUNKS, MAX_HUNK_BYTES)
    else {
        return false;
    };
    if history.missing_objects
        || history.truncated
        || history.hunks.iter().any(|hunk| hunk.text.is_none())
    {
        return false;
    }
    let old_lines = old_content
        .split(|byte| *byte == b'\n')
        .skip(old_start.saturating_sub(1) as usize)
        .take((old_end - old_start + 1) as usize)
        .collect::<Vec<_>>();
    if old_lines.is_empty() {
        return false;
    }
    let mut kept = 0usize;
    for (offset, _) in old_lines.iter().enumerate() {
        match map_line_forward(&history.hunks, old_start + offset as i64) {
            Some(Forward::Kept { new_line }) => {
                if new_line < new_start || new_line > new_end {
                    return false;
                }
                kept += 1;
            }
            Some(Forward::Rewritten {
                replacement: Some(new_line),
            }) => {
                if new_line < new_start || new_line > new_end {
                    return false;
                }
            }
            _ => return false,
        }
    }
    kept > 0
}

/// Normalized code body: strip comments and whitespace, drop empty lines.
/// Name survival alone never satisfies this comparison.
fn code_identity(content: &[u8], start: i64, end: i64) -> Vec<u8> {
    let mut result = Vec::new();
    let mut in_block_comment = false;
    for (index, line) in content
        .split(|byte| byte == &b'\n')
        .enumerate()
        .skip(start.saturating_sub(1) as usize)
    {
        if index as i64 > end {
            break;
        }
        let mut code = Vec::new();
        let mut iterator = line.iter().copied().peekable();
        while let Some(byte) = iterator.next() {
            if in_block_comment {
                if byte == b'*' && iterator.peek() == Some(&b'/') {
                    iterator.next();
                    in_block_comment = false;
                }
                continue;
            }
            if byte == b'/' && iterator.peek() == Some(&b'/') {
                break;
            }
            if byte == b'/' && iterator.peek() == Some(&b'*') {
                iterator.next();
                in_block_comment = true;
                continue;
            }
            if byte == b'"' || byte == b'\'' {
                let quote = byte;
                code.push(byte);
                while let Some(inner) = iterator.next() {
                    code.push(inner);
                    if inner == b'\\' {
                        if let Some(escaped) = iterator.next() {
                            code.push(escaped);
                        }
                    } else if inner == quote {
                        break;
                    }
                }
                continue;
            }
            code.push(byte);
        }
        let trimmed: Vec<u8> = code
            .iter()
            .copied()
            .skip_while(|byte| byte.is_ascii_whitespace())
            .collect();
        let trimmed: Vec<u8> = trimmed
            .iter()
            .copied()
            .rev()
            .skip_while(|byte| byte.is_ascii_whitespace())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if !trimmed.is_empty() {
            result.extend_from_slice(&trimmed);
            result.push(b'\n');
        }
    }
    result
}
