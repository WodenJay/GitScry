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
    pub(crate) parent_count: usize,
    pub(crate) patch: Option<PatchExcerpt>,
}

pub(crate) struct Report {
    pub(crate) start_revision: String,
    pub(crate) endpoint: String,
    pub(crate) path: Vec<u8>,
    pub(crate) line: usize,
    pub(crate) final_state: FinalState,
    pub(crate) stopped_at: Option<String>,
    pub(crate) stop_reason: Option<StopReason>,
    pub(crate) last_location: Option<Location>,
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
    line: usize,
    at: String,
    to_rev: Option<String>,
    max_commits: Option<usize>,
    options: Options,
) -> Result<Outcome, AppError> {
    let path = normalize_git_path_string(&path);
    let repository = Repository::discover()?;
    let (context, target) =
        Context::prepare_target(&repository, Some(&at), options.scope, |revision| {
            repository.pin_fate_target(revision, &path, line)
        })?;
    let session = &context.session;

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
                let history = scope::reachable_history(session, &repository, &head)?;
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
    let coverage = scope::reachable_history(session, &repository, &traversal_endpoint)?;
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
                final_state: FinalState::ReachedEndpoint,
                stopped_at: None,
                stop_reason: None,
                last_location: Some(Location {
                    path: target.path,
                    line: i64::from(u32::try_from(target.line).unwrap_or(0)),
                }),
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
            final_state,
            stopped_at,
            stop_reason,
            last_location,
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
