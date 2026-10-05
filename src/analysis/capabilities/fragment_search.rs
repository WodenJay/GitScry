use std::{cmp::Reverse, collections::BTreeMap, path::PathBuf};

use crate::{
    analysis::{
        CodeDirection, SearchScopeInfo, patch,
        query::{Context, Options, Outcome, QueryReport},
        retrieval::code_fragment::{self, Fragment, Span},
    },
    app::AppError,
    cache::CodeHunk,
};

pub(crate) struct Report {
    pub(crate) occurrences: Vec<Occurrence>,
    pub(crate) matched_count: usize,
    pub(crate) truncated: bool,
    pub(crate) scope: Option<SearchScopeInfo>,
}

pub(crate) struct Occurrence {
    pub(crate) commit_id: String,
    pub(crate) path: Vec<u8>,
    pub(crate) direction: CodeDirection,
    pub(crate) start_line: usize,
    pub(crate) end_line: usize,
    pub(crate) content: Vec<u8>,
    pub(crate) surrounding: Surrounding,
}

pub(crate) struct Surrounding {
    pub(crate) old_path: Option<Vec<u8>>,
    pub(crate) new_path: Option<Vec<u8>>,
    pub(crate) old_hunk_start: i64,
    pub(crate) new_hunk_start: i64,
    /// Zero-based diff row offsets within this cached hunk.
    pub(crate) first_diff_row: usize,
    pub(crate) last_diff_row: usize,
    pub(crate) text: Option<Vec<u8>>,
    pub(crate) truncated: bool,
}

// Same recent-first and positional tie-breaks as changed-line search.
type Position = (
    Reverse<i64>,
    String,
    Vec<u8>,
    i64,
    i64,
    usize,
    CodeDirection,
);

pub(in crate::analysis) fn execute(
    input: PathBuf,
    path: Option<String>,
    direction: Option<CodeDirection>,
    options: Options,
) -> Result<Outcome, AppError> {
    let fragment = Fragment::read(&input)?;
    let context = Context::open(options.scope)?;
    let normalized_path = path.as_deref().map(super::normalize_git_path_string);
    let mut selected = BTreeMap::new();
    let mut matched_count = 0usize;
    let scan = |hunk: CodeHunk| {
        code_fragment::visit_hunk(
            &hunk,
            &fragment,
            normalized_path.as_deref().map(str::as_bytes),
            direction,
            |span| {
                matched_count = matched_count.checked_add(1).ok_or_else(|| {
                    AppError::operational("error: fragment-search match count overflow")
                })?;
                let path = match span.direction {
                    CodeDirection::Added => &hunk.new_path,
                    CodeDirection::Removed => &hunk.old_path,
                }
                .as_ref()
                .expect("matching source side has a path");
                let key: Position = (
                    Reverse(hunk.commit_time),
                    hunk.oid.clone(),
                    path.clone(),
                    hunk.change_ordinal,
                    hunk.hunk_ordinal,
                    span.rows[0].order,
                    span.direction,
                );
                if selected.len() < options.limit
                    || selected
                        .last_key_value()
                        .is_some_and(|(worst, _)| key < *worst)
                {
                    let occurrence = assemble(&hunk, path, span);
                    selected.insert(key, occurrence);
                    if selected.len() > options.limit {
                        selected.pop_last();
                    }
                }
                Ok(())
            },
        )
    };
    match context.filter() {
        Some(filter) => context.session.scan_code_hunks_scoped(filter, scan)?,
        None => context.session.scan_code_hunks(scan)?,
    }
    let mut occurrences: Vec<_> = selected.into_values().collect();
    // Presentation budget only: matched bytes and locators are never bounded.
    let mut remaining = patch::MAX_RESULT_EXCERPT_BYTES;
    for occurrence in &mut occurrences {
        let surrounding = &mut occurrence.surrounding;
        if let Some(text) = &mut surrounding.text {
            let allowed = remaining.min(text.len());
            surrounding.truncated |= allowed < text.len();
            text.truncate(allowed);
            remaining -= allowed;
            if text.is_empty() {
                surrounding.text = None;
            }
        }
    }
    Ok(context.finish(QueryReport::FragmentSearch(Report {
        occurrences,
        matched_count,
        truncated: matched_count > options.limit,
        scope: None,
    })))
}

fn assemble(hunk: &CodeHunk, path: &[u8], span: Span<'_>) -> Occurrence {
    let first = span.rows.first().expect("non-empty fragment span");
    let last = span.rows.last().expect("non-empty fragment span");
    let first_diff_row = first.order.saturating_sub(3);
    let last_diff_row = (last.order + 3).min(span.diff_rows.len() - 1);
    let mut text = Vec::new();
    let mut truncated = false;
    for row in &span.diff_rows[first_diff_row..=last_diff_row] {
        let allowed = row.len().min(patch::MAX_EXCERPT_BYTES - text.len());
        text.extend_from_slice(&row[..allowed]);
        truncated |= allowed < row.len();
    }
    Occurrence {
        commit_id: hunk.oid.clone(),
        path: path.to_vec(),
        direction: span.direction,
        start_line: first.number,
        end_line: last.number,
        content: span
            .rows
            .iter()
            .flat_map(|row| row.content.iter().copied())
            .collect(),
        surrounding: Surrounding {
            old_path: hunk.old_path.clone(),
            new_path: hunk.new_path.clone(),
            old_hunk_start: hunk.old_start,
            new_hunk_start: hunk.new_start,
            first_diff_row,
            last_diff_row,
            text: Some(text),
            truncated,
        },
    }
}
