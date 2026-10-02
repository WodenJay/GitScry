//! Local exact-identity signals and one shared, bounded changed-code verification pass.
use super::context::{Category, ContentMatch, Report, Suggestion};
use crate::{
    analysis::{Citation, retrieval},
    app::AppError,
    cache::{QuerySession, SearchFilter},
    git::CurrentHunk,
};
use std::collections::{BTreeMap, BTreeSet};

const LOCAL_LIMIT: usize = 512;
const SIGNAL_LIMIT: usize = 24;
const CANDIDATE_LIMIT: usize = 128;
const MATCH_LIMIT: usize = 16;
const EXCERPT_LIMIT: usize = 240;

// Identity is kept whole and case-sensitive. Normalized pieces only decide whether
// an identifier is distinctive; they never establish the historical match.
fn signals(text: &[u8]) -> BTreeSet<String> {
    let Ok(text) = std::str::from_utf8(text) else {
        return BTreeSet::new();
    };
    let mut signals = BTreeSet::new();
    for identity in text.split(|c: char| !c.is_alphanumeric() && c != '_') {
        let normalized = retrieval::tokenize(identity);
        if identity.len() >= 8
            && identity.len() <= 96
            && (normalized.len() >= 2 || identity.contains('_'))
            && !matches!(
                identity,
                "to_string"
                    | "to_owned"
                    | "is_empty"
                    | "Some"
                    | "unwrap_or_default"
                    | "assert_eq"
                    | "assert_ne"
            )
        {
            signals.insert(identity.to_owned());
        }
    }
    // Quoted nontrivial literals remain intact, including punctuation and case.
    for quote in ['\"', '\''] {
        let mut pieces = text.split(quote);
        pieces.next();
        while let Some(literal) = pieces.next() {
            if literal.len() >= 6
                && literal.len() <= 96
                && literal.chars().any(char::is_alphabetic)
                && !literal.chars().any(char::is_whitespace)
                && !matches!(
                    literal,
                    "string"
                        | "default"
                        | "true"
                        | "false"
                        | "success"
                        | "message"
                        | "result"
                        | "status"
                        | "error"
                )
            {
                signals.insert(format!("{quote}{literal}{quote}"));
            }
            pieces.next();
        }
    }
    signals
}

pub(super) fn discover(
    session: &QuerySession,
    report: &mut Report,
    scope: Option<&SearchFilter>,
) -> Result<Vec<(usize, i64, Suggestion)>, AppError> {
    let mut locals: Vec<(&CurrentHunk, BTreeSet<String>)> = Vec::new();
    for hunk in &report.input.content {
        if locals.len() >= LOCAL_LIMIT {
            report.omitted_content_bases += 1;
            continue;
        }
        let mut extracted = signals(&hunk.text);
        if extracted.len() > SIGNAL_LIMIT {
            report.omitted_content_signals += extracted.len() - SIGNAL_LIMIT;
            extracted = extracted.into_iter().take(SIGNAL_LIMIT).collect();
        }
        if extracted.len() >= 2 {
            locals.push((hunk, extracted));
        }
    }
    if locals.is_empty() {
        return Ok(Vec::new());
    }
    let mut lookup: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, (_, signals)) in locals.iter().enumerate() {
        for signal in signals {
            lookup.entry(signal).or_default().push(index);
        }
    }
    let mut candidates: BTreeMap<String, (i64, Suggestion, BTreeSet<usize>)> = BTreeMap::new();
    let mut candidate_omitted = false;
    let (truncated, oversized) = session.scan_code_hunks_bounded(scope, |hunk| {
        let mut old_line = usize::try_from(hunk.old_start).map_err(|_| AppError::operational("error: invalid historical hunk location"))?;
        let mut new_line = usize::try_from(hunk.new_start).map_err(|_| AppError::operational("error: invalid historical hunk location"))?;
        for raw in hunk.text.split(|b| *b == b'\n') {
            let (added, line_number, path) = match raw.first() {
                Some(b' ') => { old_line += 1; new_line += 1; continue; }
                Some(b'+') => { let line = new_line; new_line += 1; (true, line, &hunk.new_path) }
                Some(b'-') => { let line = old_line; old_line += 1; (false, line, &hunk.old_path) }
                _ => continue,
            };
            let Some(path) = path else { continue; };
            let text = &raw[1..];
            let mut hits: BTreeMap<usize, Vec<String>> = BTreeMap::new();
            for signal in signals(text) {
                if let Some(indices) = lookup.get(signal.as_str()) {
                    for index in indices { hits.entry(*index).or_default().push(signal.clone()); }
                }
            }
            for (index, shared) in hits.into_iter().filter(|(_, shared)| shared.len() >= 2) {
                if !candidates.contains_key(&hunk.oid) && candidates.len() >= CANDIDATE_LIMIT {
                    candidate_omitted = true;
                    continue;
                }
                let local = locals[index].0;
                let (_, suggestion, bases) = candidates.entry(hunk.oid.clone()).or_insert_with(|| (hunk.commit_time, Suggestion {
                    category: Category::HistoricalChange, path: path.clone(),
                    associated_current_paths: Vec::new(), basis: vec!["Exact distinctive identities verified in historical changed-code lines; direction is material, not a same-kind or review conclusion.".to_owned()],
                    selection_routes: vec!["changed_code"], citations: Vec::new(), supporting_count: 1,
                    citations_truncated: false, content_matches: Vec::new(), content_matches_truncated: false,
                }, BTreeSet::new()));
                bases.insert(index);
                if !suggestion.associated_current_paths.contains(&local.path) {
                    suggestion.associated_current_paths.push(local.path.clone());
                }
                if suggestion.content_matches.len() >= MATCH_LIMIT {
                    suggestion.content_matches_truncated = true;
                    continue;
                }
                suggestion.content_matches.push(ContentMatch {
                    current_path: local.path.clone(), current_added: local.added,
                    current_old_start: local.old_start, current_new_start: local.new_start,
                    historical_path: path.clone(), historical_added: added,
                    historical_line: line_number, historical_old_start: hunk.old_start,
                    historical_new_start: hunk.new_start, signals: shared,
                    excerpt: text[..text.len().min(EXCERPT_LIMIT)].to_vec(), excerpt_truncated: text.len() > EXCERPT_LIMIT,
                });
            }
        }
        Ok(())
    })?;
    report.historical_content_truncated = truncated || candidate_omitted;
    report.omitted_historical_hunks = oversized;
    let mut found = Vec::new();
    for (oid, (time, mut suggestion, bases)) in candidates {
        suggestion.associated_current_paths.sort();
        suggestion.basis.push(format!(
            "{} local current hunk sides support this commit",
            bases.len()
        ));
        suggestion.citations.push(Citation::new(
            oid.clone(),
            retrieval::commit_text(session, &oid)?
                .map(|(subject, _)| subject)
                .unwrap_or_default(),
        ));
        found.push((suggestion.associated_current_paths.len(), time, suggestion));
    }
    Ok(found)
}
