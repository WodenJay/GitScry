//! Forward fate tracking for one historical line through linear history.
//!
//! The tracked line is validated at the starting revision, then mapped forward
//! through cached endpoint-reachable history. Unchanged lines follow preceding
//! insertions and deletions and detected renames without becoming events; a
//! rewritten line is one bounded replacement event and then tracking stops.
//! Unsupported or missing correspondence stops explicitly as unknown.

use std::collections::{BTreeSet, HashSet};

use crate::cache::{PatchHistoryHunk, QuerySession};
use crate::git::Repository;

use super::normalize_git_path_string;
use crate::analysis::query::{Context, Options, Outcome, QueryReport, scope};
use crate::analysis::{PatchExcerpt, SearchScopeInfo};
use crate::git::SymbolSelection;
use crate::{app::AppError, cache::followups::ForwardCommit};

const MAX_HUNKS: usize = 4_096;
const MAX_HUNK_BYTES: usize = 16 * 1024;
const MAX_MOVE_ASSOCIATIONS: usize = 8;

fn bounded_associations(
    associations: impl IntoIterator<Item = Association>,
) -> (Vec<Association>, bool) {
    let mut bounded = BTreeSet::new();
    let mut truncated = false;
    for association in associations {
        bounded.insert(association);
        if bounded.len() > MAX_MOVE_ASSOCIATIONS {
            bounded.pop_last();
            truncated = true;
        }
    }
    (bounded.into_iter().collect(), truncated)
}

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

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Location {
    pub(crate) path: Vec<u8>,
    pub(crate) line: i64,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Association {
    pub(crate) location: Location,
    pub(crate) span: Option<(i64, i64)>,
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
    pub(crate) associations: Vec<Association>,
    pub(crate) associations_truncated: bool,
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
    if line.is_some() == symbol.is_some() {
        return Err(AppError::input(
            "exactly one of --line or --symbol must be provided",
        ));
    }
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
    let mut warnings = target.warnings.clone();
    let (endpoint, traversal_endpoint, endpoint_frontier) = resolve_endpoint(
        session,
        repository,
        context,
        to_rev.as_deref(),
        &mut warnings,
    )?;

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
                associations: Vec::new(),
                associations_truncated: false,
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
    let mut associations = Vec::new();
    let mut associations_truncated = false;
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
        let line_mapping = map_line_forward(&history.hunks, state.line);
        if matches!(
            &line_mapping,
            Some(Forward::Deleted | Forward::Rewritten { .. })
        ) {
            let old_content = repository.read_blob(&last_revision, &state.path)?;
            let context = old_content
                .as_deref()
                .and_then(|content| line_move_context(content, state.line, &state.path));
            let mut move_candidates = BTreeSet::new();
            if let Some(context) = context {
                for candidate_change in &changes {
                    let Some(candidate_path) = candidate_change.new_path.as_ref() else {
                        continue;
                    };
                    if candidate_change.status.starts_with('D')
                        || candidate_change.status.starts_with('T')
                    {
                        continue;
                    }
                    let Some(candidate_content) =
                        repository.read_blob(&node.oid, candidate_path)?
                    else {
                        continue;
                    };
                    let candidate_lines = source_line_bytes(&candidate_content);
                    for candidate_index in 0..candidate_lines.len() {
                        if line_context_matches(
                            &candidate_content,
                            &candidate_lines,
                            candidate_index,
                            &context,
                            candidate_path,
                        ) {
                            let Ok(candidate_line) = i64::try_from(candidate_index + 1) else {
                                continue;
                            };
                            move_candidates.insert((candidate_path.clone(), candidate_line));
                            if move_candidates.len() > MAX_MOVE_ASSOCIATIONS + 1 {
                                move_candidates.pop_last();
                            }
                        }
                    }
                }
            }
            if move_candidates.len() == 1 {
                let (candidate_path, candidate_line) = move_candidates
                    .into_iter()
                    .next()
                    .expect("one move candidate was counted");
                events.push(Event {
                    commit_id: node.oid.clone(),
                    subject: session.forward_subject(&node.oid)?,
                    commit_time: node.commit_time,
                    relationship: "move",
                    before: state.clone(),
                    before_span: None,
                    after_span: None,
                    after: Some(Location {
                        path: candidate_path.clone(),
                        line: candidate_line,
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
                state.path = candidate_path;
                state.line = candidate_line;
                last_revision = node.oid.clone();
                continue;
            }
            if move_candidates.len() > 1 {
                let candidates = move_candidates.into_iter().map(|(path, line)| Association {
                    location: Location { path, line },
                    span: None,
                });
                (associations, associations_truncated) = bounded_associations(candidates);
                stop = Some((
                    node.oid.clone(),
                    FinalState::Unknown,
                    StopReason {
                        code: "ambiguous_move_candidates",
                        explanation: "the unchanged line has multiple strict-context matches; its move cannot be established uniquely",
                    },
                ));
                break;
            }
        }

        match line_mapping {
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
            associations,
            associations_truncated,
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
const MOVE_CONTEXT_RADIUS: usize = 3;
const MIN_MOVE_CONTEXT_LINES: usize = 3;

struct LineMoveContext {
    lines: Vec<Vec<u8>>,
    target_offset: usize,
    source: Vec<u8>,
    language: Option<&'static str>,
    can_normalize_indentation: bool,
}

fn source_line_bytes(content: &[u8]) -> Vec<Vec<u8>> {
    if content.is_empty() {
        return Vec::new();
    }
    let mut lines = content
        .split(|byte| *byte == b'\n')
        .map(<[u8]>::to_vec)
        .collect::<Vec<_>>();
    if content.ends_with(b"\n") {
        lines.pop();
    }
    lines
}

fn join_source_lines(lines: &[Vec<u8>]) -> Vec<u8> {
    let mut source = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            source.push(b'\n');
        }
        source.extend_from_slice(line);
    }
    source
}

fn line_move_context(content: &[u8], line: i64, path: &[u8]) -> Option<LineMoveContext> {
    let lines = source_line_bytes(content);
    let target_index = usize::try_from(line.checked_sub(1)?).ok()?;
    let target = lines.get(target_index)?;
    if target.iter().all(u8::is_ascii_whitespace) {
        return None;
    }
    let start = target_index.saturating_sub(MOVE_CONTEXT_RADIUS);
    let end = target_index
        .saturating_add(MOVE_CONTEXT_RADIUS + 1)
        .min(lines.len());
    let context_lines = lines[start..end].to_vec();
    if context_lines
        .iter()
        .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
        .count()
        < MIN_MOVE_CONTEXT_LINES
    {
        return None;
    }
    let language = source_language_for_path(path);
    Some(LineMoveContext {
        source: join_source_lines(&context_lines),
        lines: context_lines,
        target_offset: target_index - start,
        language,
        can_normalize_indentation: language.is_some_and(|language| {
            supports_indentation_normalization(language)
                && !contains_multiline_literal(content, language)
        }),
    })
}

fn line_context_matches(
    content: &[u8],
    lines: &[Vec<u8>],
    target_index: usize,
    context: &LineMoveContext,
    path: &[u8],
) -> bool {
    let Some(start) = target_index.checked_sub(context.target_offset) else {
        return false;
    };
    let Some(end) = start.checked_add(context.lines.len()) else {
        return false;
    };
    let Some(candidate_lines) = lines.get(start..end) else {
        return false;
    };
    let candidate_language = source_language_for_path(path);
    let can_normalize_indentation = context.can_normalize_indentation
        && candidate_language == context.language
        && context
            .language
            .is_some_and(|language| !contains_multiline_literal(content, language));
    strict_source_equal(
        &context.source,
        &join_source_lines(candidate_lines),
        context.language,
        can_normalize_indentation,
    )
}

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
    let mut qualified_name = selection.qualified_name.clone();
    let mut symbol_language = selection.language.clone();

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
                associations: Vec::new(),
                associations_truncated: false,
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
    let mut state_body_span = target.symbol_body_span;
    let mut last_revision = target.revision.clone();
    let mut events: Vec<Event> = Vec::new();
    let mut associations = Vec::new();
    let mut associations_truncated = false;
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
                let strict_target = repository
                    .read_blob(&last_revision, &state_path)?
                    .as_deref()
                    .and_then(|content| symbol_source_bytes(content, (state_start, state_end)));
                let mut move_candidates = Vec::new();
                let mut exact_candidates = 0usize;
                if let Some(strict_target) = strict_target {
                    let symbol_name = qualified_name
                        .rsplit("::")
                        .next()
                        .unwrap_or(&qualified_name);
                    for candidate_change in &changes {
                        let Some(candidate_path) = candidate_change.new_path.as_ref() else {
                            continue;
                        };
                        if candidate_path == &state_path
                            || candidate_change.status.starts_with('D')
                            || candidate_change.status.starts_with('T')
                        {
                            continue;
                        }
                        let Ok(candidate_path_text) = std::str::from_utf8(candidate_path) else {
                            continue;
                        };
                        let Some(candidate_content) =
                            repository.read_blob(&node.oid, candidate_path)?
                        else {
                            continue;
                        };
                        let Ok(candidate) = repository.locate_symbol(
                            &candidate_content,
                            symbol_name,
                            candidate_path_text,
                        ) else {
                            continue;
                        };
                        let Ok(candidate_start) = i64::try_from(candidate.start_line) else {
                            continue;
                        };
                        let Ok(candidate_end) = i64::try_from(candidate.end_line) else {
                            continue;
                        };
                        let Some(candidate_bytes) = symbol_source_bytes(
                            &candidate_content,
                            (candidate_start, candidate_end),
                        ) else {
                            continue;
                        };
                        if !strict_source_equal(
                            &strict_target,
                            &candidate_bytes,
                            Some(symbol_language.as_str()),
                            candidate.selection.language == symbol_language,
                        ) {
                            continue;
                        }
                        exact_candidates += 1;
                        move_candidates.push((
                            candidate_path.clone(),
                            candidate,
                            candidate_start,
                            candidate_end,
                        ));
                        move_candidates.sort_by(|left, right| {
                            left.0
                                .cmp(&right.0)
                                .then_with(|| left.2.cmp(&right.2))
                                .then_with(|| left.3.cmp(&right.3))
                        });
                        if move_candidates.len() > MAX_MOVE_ASSOCIATIONS + 1 {
                            move_candidates.pop();
                        }
                    }
                }
                if exact_candidates == 1
                    && span_was_deleted(
                        session,
                        &node.oid,
                        change.ordinal,
                        (state_start, state_end),
                    ) == Some(true)
                {
                    if let Some((candidate_path, candidate, new_start, new_end)) =
                        move_candidates.pop()
                    {
                        push_symbol_event(
                            &mut events,
                            session,
                            &node.oid,
                            node.commit_time,
                            "move",
                            node.parents.len(),
                            &state_path,
                            state_start,
                            state_end,
                            Some(&candidate_path),
                            new_start,
                            new_end,
                            options.patch,
                            change.ordinal,
                        )?;
                        state_path = candidate_path;
                        state_start = new_start;
                        state_end = new_end;
                        state_body_span = candidate.body_span;
                        symbol_language = candidate.selection.language.clone();
                        qualified_name = candidate.selection.qualified_name.clone();
                        last_revision = node.oid.clone();
                        continue;
                    }
                }
                if exact_candidates > 1
                    && span_was_deleted(
                        session,
                        &node.oid,
                        change.ordinal,
                        (state_start, state_end),
                    ) == Some(true)
                {
                    let candidates =
                        move_candidates
                            .into_iter()
                            .map(|(path, _, start, end)| Association {
                                location: Location { path, line: start },
                                span: Some((start, end)),
                            });
                    (associations, associations_truncated) = bounded_associations(candidates);
                    stop = Some((
                        node.oid.clone(),
                        FinalState::Unknown,
                        StopReason {
                            code: "ambiguous_move_candidates",
                            explanation: "the unchanged symbol has multiple strict-source matches; its move cannot be established uniquely",
                        },
                    ));
                    break;
                }
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

        let old_content_for_move = repository.read_blob(&last_revision, &state_path)?;
        let complete_target_matches = old_content_for_move
            .as_deref()
            .and_then(|old_content| {
                let old_source = symbol_source_bytes(old_content, (state_start, state_end))?;
                let new_source = symbol_source_bytes(&new_content, (new_start, new_end))?;
                Some(strict_source_equal(
                    &old_source,
                    &new_source,
                    Some(symbol_language.as_str()),
                    new_location.selection.language == symbol_language,
                ))
            })
            .unwrap_or(false);
        if complete_target_matches
            && span_was_deleted(session, &node.oid, change.ordinal, (state_start, state_end))
                == Some(true)
        {
            let mut alternatives = BTreeSet::from([Association {
                location: Location {
                    path: new_path.clone(),
                    line: new_start,
                },
                span: Some((new_start, new_end)),
            }]);
            if let Some(strict_target) = old_content_for_move
                .as_deref()
                .and_then(|content| symbol_source_bytes(content, (state_start, state_end)))
            {
                let symbol_name = qualified_name
                    .rsplit("::")
                    .next()
                    .unwrap_or(&qualified_name);
                for candidate_change in &changes {
                    let Some(candidate_path) = candidate_change.new_path.as_ref() else {
                        continue;
                    };
                    if candidate_path == &new_path
                        || candidate_change.status.starts_with('D')
                        || candidate_change.status.starts_with('T')
                    {
                        continue;
                    }
                    let Ok(candidate_path_text) = std::str::from_utf8(candidate_path) else {
                        continue;
                    };
                    let Some(candidate_content) =
                        repository.read_blob(&node.oid, candidate_path)?
                    else {
                        continue;
                    };
                    let Ok(candidate) = repository.locate_symbol(
                        &candidate_content,
                        symbol_name,
                        candidate_path_text,
                    ) else {
                        continue;
                    };
                    let Ok(candidate_start) = i64::try_from(candidate.start_line) else {
                        continue;
                    };
                    let Ok(candidate_end) = i64::try_from(candidate.end_line) else {
                        continue;
                    };
                    let Some(candidate_bytes) =
                        symbol_source_bytes(&candidate_content, (candidate_start, candidate_end))
                    else {
                        continue;
                    };
                    if strict_source_equal(
                        &strict_target,
                        &candidate_bytes,
                        Some(symbol_language.as_str()),
                        candidate.selection.language == symbol_language,
                    ) {
                        alternatives.insert(Association {
                            location: Location {
                                path: candidate_path.clone(),
                                line: candidate_start,
                            },
                            span: Some((candidate_start, candidate_end)),
                        });
                        if alternatives.len() > MAX_MOVE_ASSOCIATIONS + 1 {
                            alternatives.pop_last();
                        }
                    }
                }
            }
            if alternatives.len() > 1 {
                (associations, associations_truncated) = bounded_associations(alternatives);
                stop = Some((
                    node.oid.clone(),
                    FinalState::Unknown,
                    StopReason {
                        code: "ambiguous_move_candidates",
                        explanation: "the unchanged symbol has multiple strict-source matches; its move cannot be established uniquely",
                    },
                ));
                break;
            }
            push_symbol_event(
                &mut events,
                session,
                &node.oid,
                node.commit_time,
                "move",
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
            state_path = new_path.clone();
            state_start = new_start;
            state_end = new_end;
            state_body_span = new_location.body_span;
            symbol_language = new_location.selection.language.clone();
            qualified_name = new_location.selection.qualified_name.clone();
            last_revision = node.oid.clone();
            continue;
        }

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

        // Name resolution chooses the candidate; diff correspondence must still
        // map every declaration line and preserve actual body code.
        let new_body_span = new_location.body_span;
        let Some(old_content) = old_content_for_move else {
            stop = Some((
                node.oid.clone(),
                FinalState::Unknown,
                StopReason {
                    code: "symbol_source_unavailable",
                    explanation: "the tracked file's previous contents are unavailable at this commit",
                },
            ));
            break;
        };
        let correspondence = spans_map_forward(
            session,
            &node.oid,
            change.ordinal,
            SymbolSource {
                span: (state_start, state_end),
                body_span: state_body_span,
                content: &old_content,
                language: &symbol_language,
            },
            SymbolSource {
                span: (new_start, new_end),
                body_span: new_body_span,
                content: &new_content,
                language: &new_location.selection.language,
            },
        );
        let old_identity = code_identity(
            &old_content,
            (state_start, state_end),
            state_body_span,
            &symbol_language,
        );
        let new_identity = code_identity(
            &new_content,
            (new_start, new_end),
            new_body_span,
            &new_location.selection.language,
        );
        let old_symbol_identity = code_identity(
            &old_content,
            (state_start, state_end),
            None,
            &symbol_language,
        );
        let new_symbol_identity = code_identity(
            &new_content,
            (new_start, new_end),
            None,
            &new_location.selection.language,
        );
        let symbol_modified = old_symbol_identity != new_symbol_identity;
        let unchanged_body = old_identity == new_identity;
        let Some(unchanged_correspondence) = correspondence else {
            stop = Some((
                node.oid.clone(),
                FinalState::Unknown,
                StopReason {
                    code: "unreliable_boundary_correspondence",
                    explanation: "diff correspondence could not map the symbol boundaries uniquely across this change",
                },
            ));
            break;
        };
        if !unchanged_correspondence {
            let reason = if unchanged_body {
                StopReason {
                    code: "unreliable_boundary_correspondence",
                    explanation: "the declaration has no unchanged actual body code to establish continuity",
                }
            } else {
                StopReason {
                    code: "symbol_body_rewritten",
                    explanation: "the declaration body was rewritten; unchanged body code no longer establishes continuity",
                }
            };
            stop = Some((node.oid.clone(), FinalState::Unknown, reason));
            break;
        }
        if symbol_modified {
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
        state_body_span = new_body_span;
        symbol_language = new_location.selection.language.clone();
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
            associations,
            associations_truncated,
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

/// Require every old declaration line to map into the new span and at least one actual body-code
/// line to survive unchanged. Name resolution alone cannot establish continuity.
struct SymbolSource<'a> {
    span: (i64, i64),
    body_span: Option<crate::git::SymbolBodySpan>,
    content: &'a [u8],
    language: &'a str,
}

fn spans_map_forward(
    session: &crate::cache::QuerySession,
    oid: &str,
    change_ordinal: i64,
    old: SymbolSource<'_>,
    new: SymbolSource<'_>,
) -> Option<bool> {
    let (old_start, old_end) = old.span;
    let (new_start, new_end) = new.span;
    let history = session
        .patch_history_for_change(oid, change_ordinal, MAX_HUNKS, MAX_HUNK_BYTES)
        .ok()?;
    if history.missing_objects
        || history.truncated
        || history.hunks.iter().any(|hunk| hunk.text.is_none())
    {
        return None;
    }
    let old_body_lines = old
        .body_span
        .and_then(|body| body_code_lines(old.content, body, old.language));
    let new_body_lines = new
        .body_span
        .and_then(|body| body_code_lines(new.content, body, new.language));
    let mut unchanged_body_code = false;
    for old_line in old_start..=old_end {
        match map_line_forward(&history.hunks, old_line)? {
            Forward::Kept { new_line } => {
                if new_line < new_start || new_line > new_end {
                    return None;
                }
                if let (Some(old_body), Some(old_lines), Some(new_body), Some(new_lines)) = (
                    old.body_span,
                    &old_body_lines,
                    new.body_span,
                    &new_body_lines,
                ) && has_body_code_on_line(old_body, old_lines, old_line)
                    && has_body_code_on_line(new_body, new_lines, new_line)
                {
                    unchanged_body_code = true;
                }
            }
            Forward::Rewritten {
                replacement: Some(new_line),
            } => {
                if new_line < new_start || new_line > new_end {
                    return None;
                }
            }
            Forward::Deleted | Forward::Rewritten { replacement: None } => return None,
        }
    }
    Some(unchanged_body_code)
}

fn body_code_lines(
    content: &[u8],
    body: crate::git::SymbolBodySpan,
    language: &str,
) -> Option<Vec<Vec<u8>>> {
    Some(normalized_code_lines(
        content.get(body.start_byte..body.end_byte)?,
        language,
    ))
}

fn has_body_code_on_line(body: crate::git::SymbolBodySpan, lines: &[Vec<u8>], line: i64) -> bool {
    let Ok(start) = i64::try_from(body.start_line) else {
        return false;
    };
    let Ok(end) = i64::try_from(body.end_line) else {
        return false;
    };
    if line < start || line > end {
        return false;
    }
    let index = usize::try_from(line - start).unwrap_or(usize::MAX);
    lines.get(index).is_some_and(|line| has_actual_code(line))
}

fn has_actual_code(code: &[u8]) -> bool {
    code.iter().any(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'"' | b'\'') || !byte.is_ascii()
    })
}

fn code_identity(
    content: &[u8],
    span: (i64, i64),
    body: Option<crate::git::SymbolBodySpan>,
    language: &str,
) -> Vec<u8> {
    let source = body
        .and_then(|body| content.get(body.start_byte..body.end_byte))
        .map(<[u8]>::to_vec)
        .unwrap_or_else(|| {
            let (start, end) = span;
            content
                .split_inclusive(|byte| *byte == b'\n')
                .enumerate()
                .filter(|(index, _)| {
                    let line = i64::try_from(index + 1).unwrap_or(i64::MAX);
                    line >= start && line <= end
                })
                .flat_map(|(_, line)| line.iter().copied())
                .collect()
        });
    normalized_code_lines(&source, language)
        .into_iter()
        .filter(|line| !line.is_empty())
        .fold(Vec::new(), |mut identity, line| {
            identity.extend_from_slice(&line);
            identity.push(b'\n');
            identity
        })
}

fn span_was_deleted(
    session: &crate::cache::QuerySession,
    oid: &str,
    change_ordinal: i64,
    span: (i64, i64),
) -> Option<bool> {
    if span.0 > span.1 {
        return Some(false);
    }
    let history = session
        .patch_history_for_change(oid, change_ordinal, MAX_HUNKS, MAX_HUNK_BYTES)
        .ok()?;
    if history.missing_objects
        || history.truncated
        || history.hunks.iter().any(|hunk| hunk.text.is_none())
    {
        return None;
    }
    for line in span.0..=span.1 {
        match map_line_forward(&history.hunks, line)? {
            Forward::Deleted | Forward::Rewritten { replacement: None } => {}
            Forward::Kept { .. }
            | Forward::Rewritten {
                replacement: Some(_),
            } => {
                return Some(false);
            }
        }
    }
    Some(true)
}

fn strict_source_equal(
    left: &[u8],
    right: &[u8],
    language: Option<&str>,
    allow_indentation_normalization: bool,
) -> bool {
    if left == right {
        return true;
    }
    let Some(language) = language.filter(|_| allow_indentation_normalization) else {
        return false;
    };
    let Some(left) = normalize_source_for_move(left, language) else {
        return false;
    };
    let Some(right) = normalize_source_for_move(right, language) else {
        return false;
    };
    left == right
}

fn source_language_for_path(path: &[u8]) -> Option<&'static str> {
    let extension = std::str::from_utf8(path.rsplit(|byte| *byte == b'.').next()?).ok()?;
    match extension.to_ascii_lowercase().as_str() {
        "rs" => Some("rust"),
        "py" | "pyw" => Some("python"),
        "js" | "jsx" | "mjs" | "cjs" => Some("javascript"),
        "ts" | "tsx" | "mts" | "cts" => Some("typescript"),
        "go" => Some("go"),
        "java" => Some("java"),
        "kt" | "kts" => Some("kotlin"),
        "cc" | "cpp" | "cxx" | "hpp" | "hxx" => Some("cpp"),
        _ => None,
    }
}

fn supports_indentation_normalization(language: &str) -> bool {
    matches!(
        language,
        "rust" | "python" | "javascript" | "typescript" | "go" | "java" | "kotlin" | "cpp"
    )
}

fn contains_multiline_literal(source: &[u8], language: &str) -> bool {
    match language {
        "rust" => contains_multiline_rust_raw_string(source),
        "python" => {
            contains_multiline_delimited(source, &[b'\"', b'\"', b'\"'])
                || contains_multiline_delimited(source, b"'''")
        }
        "javascript" | "typescript" | "go" => contains_multiline_delimited(source, b"`"),
        "java" | "kotlin" => contains_multiline_delimited(source, &[b'\"', b'\"', b'\"']),
        "cpp" => contains_multiline_cpp_raw_string(source),
        _ => true,
    }
}

fn contains_multiline_delimited(source: &[u8], delimiter: &[u8]) -> bool {
    let mut search_from = 0;
    while search_from + delimiter.len() <= source.len() {
        let Some(relative_open) = source[search_from..]
            .windows(delimiter.len())
            .position(|window| window == delimiter)
        else {
            return false;
        };
        let open = search_from + relative_open;
        let body_start = open + delimiter.len();
        let mut cursor = body_start;
        while cursor + delimiter.len() <= source.len() {
            if source[cursor] == b'\\' {
                cursor = cursor.saturating_add(2);
                continue;
            }
            if source[cursor..].starts_with(delimiter) {
                if source[body_start..cursor].contains(&b'\n') {
                    return true;
                }
                search_from = cursor + delimiter.len();
                break;
            }
            cursor += 1;
        }
        if cursor + delimiter.len() > source.len() {
            return source[body_start..].contains(&b'\n');
        }
    }
    false
}

fn contains_multiline_rust_raw_string(source: &[u8]) -> bool {
    for open in 0..source.len() {
        if source[open] != b'r' {
            continue;
        }
        let mut quote = open + 1;
        while source.get(quote) == Some(&b'#') {
            quote += 1;
        }
        if source.get(quote) != Some(&b'\"') {
            continue;
        }
        let body_start = quote + 1;
        let mut closing = vec![b'\"'];
        closing.extend(std::iter::repeat_n(b'#', quote - open - 1));
        if let Some(relative_close) = source[body_start..]
            .windows(closing.len())
            .position(|window| window == closing)
            && source[body_start..body_start + relative_close].contains(&b'\n')
        {
            return true;
        }
    }
    false
}

fn contains_multiline_cpp_raw_string(source: &[u8]) -> bool {
    let mut search_from = 0;
    while let Some(relative_open) = source[search_from..]
        .windows(2)
        .position(|window| window == [b'R', b'\"'])
    {
        let open = search_from + relative_open;
        let delimiter_start = open + 2;
        let Some(relative_paren) = source[delimiter_start..]
            .iter()
            .position(|byte| *byte == b'(')
        else {
            return false;
        };
        let paren = delimiter_start + relative_paren;
        let delimiter = &source[delimiter_start..paren];
        if delimiter.len() > 16 {
            search_from = paren + 1;
            continue;
        }
        let body_start = paren + 1;
        let mut closing = vec![b')'];
        closing.extend_from_slice(delimiter);
        closing.push(b'\"');
        if let Some(relative_close) = source[body_start..]
            .windows(closing.len())
            .position(|window| window == closing)
        {
            if source[body_start..body_start + relative_close].contains(&b'\n') {
                return true;
            }
            search_from = body_start + relative_close + closing.len();
        } else {
            return source[body_start..].contains(&b'\n');
        }
    }
    false
}

fn normalize_source_for_move(source: &[u8], language: &str) -> Option<Vec<u8>> {
    if !supports_indentation_normalization(language) || contains_multiline_literal(source, language)
    {
        return None;
    }
    let chunks = source
        .split_inclusive(|byte| *byte == b'\n')
        .collect::<Vec<_>>();
    let mut lines = Vec::with_capacity(chunks.len());
    let mut newline_terminated = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        let has_newline = chunk.ends_with(b"\n");
        let mut line = if has_newline {
            &chunk[..chunk.len() - 1]
        } else {
            chunk
        };
        if has_newline {
            line = line.strip_suffix(b"\r").unwrap_or(line);
        }
        lines.push(line.to_vec());
        newline_terminated.push(has_newline);
    }
    let mut common_indent: Option<Vec<u8>> = None;
    for line in &lines {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let indent = line
            .iter()
            .take_while(|byte| matches!(byte, b' ' | 9))
            .copied()
            .collect::<Vec<_>>();
        if let Some(common) = common_indent.as_mut() {
            let shared = common
                .iter()
                .zip(&indent)
                .take_while(|(left, right)| left == right)
                .count();
            common.truncate(shared);
        } else {
            common_indent = Some(indent);
        }
    }
    let common_indent = common_indent.unwrap_or_default();
    let mut normalized = Vec::new();
    for (line, has_newline) in lines.into_iter().zip(newline_terminated) {
        let start = if line.iter().all(u8::is_ascii_whitespace) {
            line.iter()
                .take_while(|byte| matches!(byte, b' ' | 9))
                .count()
        } else {
            common_indent.len()
        };
        normalized.extend_from_slice(&line[start..]);
        if has_newline {
            normalized.push(b'\n');
        }
    }
    Some(normalized)
}

fn symbol_source_bytes(content: &[u8], span: (i64, i64)) -> Option<Vec<u8>> {
    let start = usize::try_from(span.0.checked_sub(1)?).ok()?;
    let end = usize::try_from(span.1).ok()?;
    let lines = content
        .split_inclusive(|byte| *byte == b'\n')
        .collect::<Vec<_>>();
    let source = lines.get(start..end)?.concat();
    (!source.is_empty()).then_some(source)
}

fn normalized_code_lines(content: &[u8], language: &str) -> Vec<Vec<u8>> {
    let mut result = Vec::new();
    let mut in_block_comment = false;
    let mut python_triple_quote = None;
    for line in content.split(|byte| *byte == b'\n') {
        let mut code = Vec::new();
        let mut iterator = line.iter().copied().peekable();
        while let Some(byte) = iterator.next() {
            if let Some(quote) = python_triple_quote {
                code.push(byte);
                if byte == b'\\' {
                    if let Some(escaped) = iterator.next() {
                        code.push(escaped);
                    }
                    continue;
                }
                if byte == quote && iterator.peek() == Some(&quote) {
                    let mut closing = iterator.clone();
                    closing.next();
                    if closing.peek() == Some(&quote) {
                        code.push(iterator.next().expect("triple quote has second byte"));
                        code.push(iterator.next().expect("triple quote has third byte"));
                        python_triple_quote = None;
                    }
                }
                continue;
            }
            if in_block_comment {
                if byte == b'*' && iterator.peek() == Some(&b'/') {
                    iterator.next();
                    in_block_comment = false;
                }
                continue;
            }
            if language != "python" && byte == b'/' && iterator.peek() == Some(&b'/') {
                break;
            }
            if language != "python" && byte == b'/' && iterator.peek() == Some(&b'*') {
                iterator.next();
                in_block_comment = true;
                continue;
            }
            if language == "python" && byte == b'#' {
                break;
            }
            if language == "python" && matches!(byte, b'"' | b'\'') {
                let mut opening = iterator.clone();
                if opening.peek() == Some(&byte) {
                    opening.next();
                    if opening.peek() == Some(&byte) {
                        code.extend([byte, byte, byte]);
                        iterator.next();
                        iterator.next();
                        python_triple_quote = Some(byte);
                        continue;
                    }
                }
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
        let first = code
            .iter()
            .position(|byte| !byte.is_ascii_whitespace())
            .unwrap_or(code.len());
        let last = code
            .iter()
            .rposition(|byte| !byte.is_ascii_whitespace())
            .map_or(first, |index| index + 1);
        result.push(code[first..last].to_vec());
    }
    result
}
