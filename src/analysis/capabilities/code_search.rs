use std::collections::BTreeSet;

use crate::analysis::query::{Context, Options, Outcome, QueryReport};
use crate::{
    analysis::{CodeDirection, CodeMatch, Report, ReportKind},
    app::AppError,
    cache::{CodeHunk, QuerySession, SearchFilter},
};

pub(in crate::analysis) fn execute(
    query: String,
    path: Option<String>,
    direction: Option<CodeDirection>,
    options: Options,
) -> Result<Outcome, AppError> {
    let context = Context::open(options.scope)?;
    let report = match context.filter() {
        Some(filter) => run_scoped(
            &context.session,
            &query,
            path.as_deref(),
            direction,
            options.limit,
            filter,
        )?,
        None => run(
            &context.session,
            &query,
            path.as_deref(),
            direction,
            options.limit,
        )?,
    };
    Ok(context.finish(QueryReport::Analysis(report)))
}

struct OrderedMatch {
    matched: CodeMatch,
    commit_time: i64,
    change_ordinal: i64,
    hunk_ordinal: i64,
    line_order: usize,
}

impl Ord for OrderedMatch {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .commit_time
            .cmp(&self.commit_time)
            .then_with(|| self.matched.commit_id.cmp(&other.matched.commit_id))
            .then_with(|| self.matched.path.cmp(&other.matched.path))
            .then_with(|| self.change_ordinal.cmp(&other.change_ordinal))
            .then_with(|| self.hunk_ordinal.cmp(&other.hunk_ordinal))
            .then_with(|| self.line_order.cmp(&other.line_order))
            .then_with(|| self.matched.direction.cmp(&other.matched.direction))
            .then_with(|| self.matched.line_number.cmp(&other.matched.line_number))
            .then_with(|| self.matched.line.cmp(&other.matched.line))
    }
}

impl PartialOrd for OrderedMatch {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for OrderedMatch {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for OrderedMatch {}

struct CodeSearch<'a> {
    query: &'a [u8],
    path_filter: Option<&'a [u8]>,
    direction_filter: Option<CodeDirection>,
    limit: usize,
    matches: BTreeSet<OrderedMatch>,
    matched_count: usize,
}

impl CodeSearch<'_> {
    fn scan_hunk(&mut self, hunk: CodeHunk) -> Result<(), AppError> {
        visit_hunk_matches(
            &hunk,
            self.query,
            self.path_filter,
            self.direction_filter,
            |direction, line_number, line_order, line| {
                self.record_match(&hunk, direction, line_number, line_order, line)
            },
        )
    }

    fn record_match(
        &mut self,
        hunk: &CodeHunk,
        direction: CodeDirection,
        line_number: usize,
        line_order: usize,
        line: &[u8],
    ) -> Result<(), AppError> {
        let path = match direction {
            CodeDirection::Added => hunk.new_path.as_deref(),
            CodeDirection::Removed => hunk.old_path.as_deref(),
        }
        .expect("matching source line has a path");
        self.matched_count = self
            .matched_count
            .checked_add(1)
            .ok_or_else(|| AppError::operational("error: code-search match count overflow"))?;
        let candidate = OrderedMatch {
            matched: CodeMatch {
                commit_id: hunk.oid.clone(),
                path: path.to_vec(),
                direction,
                line_number,
                line: line.to_vec(),
            },
            commit_time: hunk.commit_time,
            change_ordinal: hunk.change_ordinal,
            hunk_ordinal: hunk.hunk_ordinal,
            line_order,
        };
        if self.matches.len() < self.limit {
            self.matches.insert(candidate);
        } else if self.matches.last().is_some_and(|worst| candidate < *worst) {
            self.matches.pop_last();
            self.matches.insert(candidate);
        }
        Ok(())
    }
}

fn run(
    session: &QuerySession,
    query: &str,
    path: Option<&str>,
    direction: Option<CodeDirection>,
    limit: usize,
) -> Result<Report, AppError> {
    run_with_scope(session, query, path, direction, limit, None)
}

fn run_scoped(
    session: &QuerySession,
    query: &str,
    path: Option<&str>,
    direction: Option<CodeDirection>,
    limit: usize,
    scope: &SearchFilter,
) -> Result<Report, AppError> {
    run_with_scope(session, query, path, direction, limit, Some(scope))
}

fn run_with_scope(
    session: &QuerySession,
    query: &str,
    path: Option<&str>,
    direction: Option<CodeDirection>,
    limit: usize,
    scope: Option<&SearchFilter>,
) -> Result<Report, AppError> {
    validate_query(query)?;

    let mut search = CodeSearch {
        query: query.as_bytes(),
        path_filter: path.map(str::as_bytes),
        direction_filter: direction,
        limit,
        matches: BTreeSet::new(),
        matched_count: 0,
    };
    match scope {
        Some(scope) => session.scan_code_hunks_scoped(scope, |hunk| search.scan_hunk(hunk))?,
        None => session.scan_code_hunks(|hunk| search.scan_hunk(hunk))?,
    }
    let CodeSearch {
        matches,
        matched_count,
        ..
    } = search;
    Ok(Report {
        kind: ReportKind::CodeSearch,
        materials: Vec::new(),
        code_matches: matches.into_iter().map(|ordered| ordered.matched).collect(),
        matched_count,
        truncated: matched_count > limit,
        warnings: Vec::new(),
        notices: Vec::new(),
        patch_mode: false,
        why: None,
        scope: None,
        symbol_summary: None,
    })
}

// Stream borrowed matches so discovery can select events without retaining source text.
pub(crate) fn visit_matches(
    session: &QuerySession,
    query: &str,
    path: Option<&str>,
    direction: CodeDirection,
    scope: Option<&SearchFilter>,
    mut visit: impl FnMut(&CodeHunk, usize, &[u8]) -> Result<(), AppError>,
) -> Result<(), AppError> {
    validate_query(query)?;
    let scan = |hunk: CodeHunk| {
        visit_hunk_matches(
            &hunk,
            query.as_bytes(),
            path.map(str::as_bytes),
            Some(direction),
            |_, number, _, line| visit(&hunk, number, line),
        )
    };
    match scope {
        Some(scope) => session.scan_code_hunks_scoped(scope, scan),
        None => session.scan_code_hunks(scan),
    }
}

fn visit_hunk_matches(
    hunk: &CodeHunk,
    query: &[u8],
    path_filter: Option<&[u8]>,
    direction_filter: Option<CodeDirection>,
    mut visit: impl FnMut(CodeDirection, usize, usize, &[u8]) -> Result<(), AppError>,
) -> Result<(), AppError> {
    let mut old_line = usize::try_from(hunk.old_start)
        .map_err(|_| AppError::operational("error: cache contains an invalid old hunk line"))?;
    let mut new_line = usize::try_from(hunk.new_start)
        .map_err(|_| AppError::operational("error: cache contains an invalid new hunk line"))?;
    for (order, raw) in hunk.text.split_inclusive(|byte| *byte == b'\n').enumerate() {
        let line = raw.strip_suffix(b"\n").unwrap_or(raw);
        let (direction, number, path) = match line.first() {
            Some(b' ') => {
                advance_line(&mut old_line)?;
                advance_line(&mut new_line)?;
                continue;
            }
            Some(b'+') => {
                let number = new_line;
                advance_line(&mut new_line)?;
                (CodeDirection::Added, number, hunk.new_path.as_deref())
            }
            Some(b'-') => {
                let number = old_line;
                advance_line(&mut old_line)?;
                (CodeDirection::Removed, number, hunk.old_path.as_deref())
            }
            // Headers, metadata, and no-newline markers aren't source lines.
            _ => continue,
        };
        let Some(path) = path else {
            continue;
        };
        if direction_filter.is_some_and(|filter| filter != direction)
            || path_filter.is_some_and(|filter| filter != path)
            || !line[1..].windows(query.len()).any(|window| window == query)
        {
            continue;
        }
        visit(direction, number, order, &line[1..])?;
    }
    Ok(())
}

fn validate_query(query: &str) -> Result<(), AppError> {
    if query.is_empty()
        || query
            .chars()
            .any(|character| matches!(character, '\n' | '\r'))
    {
        return Err(AppError::input(
            "code query must be a non-empty single line",
        ));
    }
    Ok(())
}

fn advance_line(line: &mut usize) -> Result<(), AppError> {
    *line = line
        .checked_add(1)
        .ok_or_else(|| AppError::operational("error: cached hunk line number overflow"))?;
    Ok(())
}
