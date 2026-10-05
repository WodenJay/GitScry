//! Local exact-identity signals and one shared, bounded changed-code verification pass.
use super::{Category, ContentMatch, Report, Suggestion};
use crate::{
    analysis::{Citation, retrieval},
    app::AppError,
    cache::{QuerySession, SearchFilter},
    git::CurrentHunk,
};
use std::collections::{BTreeMap, BTreeSet};

// Semantic expansion is deliberately smaller than ordinary content analysis:
// at most 16 local sides, 8 signals/side, 64 chunks/vectors and 64 commits.
// Target patches cap decoded material at 64 * 64 * 16 KiB = 64 MiB.
const LOCAL_LIMIT: usize = 512;
const SIGNAL_LIMIT: usize = 24;
const CANDIDATE_LIMIT: usize = 128;
const MATCH_LIMIT: usize = 16;
const EXCERPT_LIMIT: usize = 240;
const SEMANTIC_BASE_LIMIT: usize = 16;
const SEMANTIC_CHUNK_LIMIT: usize = 64;
const SEMANTIC_CANDIDATE_LIMIT: usize = 64;
const SEMANTIC_HUNK_LIMIT: usize = 64;
const SEMANTIC_HUNK_BYTES: usize = 16 * 1024;

pub(super) fn discover(
    session: &QuerySession,
    report: &mut Report,
    scope: Option<&SearchFilter>,
    hybrid: bool,
) -> Result<Vec<(usize, i64, Suggestion)>, AppError> {
    report.semantic_requested = hybrid;
    if hybrid {
        report.limitations.push("Local semantic expansion uses at most 16 local hunk sides, the first 8 distinctive signals per side, 64 query chunks and 64 eligible vector candidates. One vector pass uses each commit's best chunk score only for retrieval. Target verification reads at most 64 hunks/commit and 16 KiB/hunk (64 MiB total); ordinary and semantic routes retain at most 192 verified commits combined. Semantic scores never bypass exact content verification or affect final association ranking.".to_owned());
    }
    let encoder = if hybrid {
        Some(crate::semantic::Encoder::load_for_query("")?.0)
    } else {
        None
    };
    let mut locals: Vec<(&CurrentHunk, BTreeSet<String>)> = Vec::new();
    for hunk in &report.input.content {
        if locals.len() >= LOCAL_LIMIT {
            report.omitted_content_bases += 1;
            continue;
        }
        let mut extracted = retrieval::distinctive_signals(&hunk.text);
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
    // Tokenize each local basis independently; chunks never pool unrelated files.
    let mut semantic = Vec::new();
    if let Some(mut encoder) = encoder {
        let mut inputs = Vec::new();
        for (_, signals) in locals.iter().take(SEMANTIC_BASE_LIMIT) {
            let query = signals
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join(" ");
            let prepared = encoder.prepare_query(&query)?;
            if inputs.len() + prepared.len() > SEMANTIC_CHUNK_LIMIT {
                report.omitted_semantic_bases += 1;
                continue;
            }
            inputs.extend(prepared);
        }
        report.omitted_semantic_bases += locals.len().saturating_sub(SEMANTIC_BASE_LIMIT);
        let vectors = encoder.embed_query_chunks(&inputs)?;
        // One eligible-vector pass, best chunk per commit; cosine only retrieves.
        semantic = session.semantic_top_k(&vectors, SEMANTIC_CANDIDATE_LIMIT, scope)?;
        report.semantic_candidates = semantic.len();
    }
    let semantic_oids: BTreeSet<&str> = semantic.iter().map(|c| c.oid.as_str()).collect();
    let mut lookup: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, (_, signals)) in locals.iter().enumerate() {
        for signal in signals {
            lookup.entry(signal).or_default().push(index);
        }
    }
    let mut candidates: BTreeMap<String, (i64, Suggestion, BTreeSet<usize>)> = BTreeMap::new();
    let mut candidate_omitted = false;
    let mut seen_hunks = BTreeSet::new();
    let mut verify = |hunk: crate::cache::CodeHunk, limit: usize| -> Result<(), AppError> {
        if !seen_hunks.insert((hunk.oid.clone(), hunk.change_ordinal, hunk.hunk_ordinal))
            && candidates.contains_key(&hunk.oid)
        {
            return Ok(());
        }
        let mut old_line = usize::try_from(hunk.old_start)
            .map_err(|_| AppError::operational("error: invalid historical hunk location"))?;
        let mut new_line = usize::try_from(hunk.new_start)
            .map_err(|_| AppError::operational("error: invalid historical hunk location"))?;
        for raw in hunk.text.split(|b| *b == b'\n') {
            let (added, line_number, path) = match raw.first() {
                Some(b' ') => {
                    old_line += 1;
                    new_line += 1;
                    continue;
                }
                Some(b'+') => {
                    let line = new_line;
                    new_line += 1;
                    (true, line, &hunk.new_path)
                }
                Some(b'-') => {
                    let line = old_line;
                    old_line += 1;
                    (false, line, &hunk.old_path)
                }
                _ => continue,
            };
            let Some(path) = path else {
                continue;
            };
            let text = &raw[1..];
            let mut hits: BTreeMap<usize, Vec<String>> = BTreeMap::new();
            for signal in retrieval::distinctive_signals(text) {
                if let Some(indices) = lookup.get(signal.as_str()) {
                    for index in indices {
                        hits.entry(*index).or_default().push(signal.clone());
                    }
                }
            }
            for (index, shared) in hits.into_iter().filter(|(_, shared)| shared.len() >= 2) {
                if !candidates.contains_key(&hunk.oid) && candidates.len() >= limit {
                    candidate_omitted = true;
                    continue;
                }
                let local = locals[index].0;
                let (_, suggestion, bases) = candidates.entry(hunk.oid.clone()).or_insert_with(|| (hunk.commit_time, Suggestion {
                    category: Category::HistoricalChange, path: path.clone(),
                    associated_current_paths: Vec::new(), basis: vec!["Exact distinctive identities verified in historical changed-code lines; direction is material, not a same-kind or review conclusion.".to_owned()],
                    selection_routes: vec!["changed_code"], citations: Vec::new(), supporting_count: 1,
                    abandonment: None, historical_followup: None, co_change: None, citations_truncated: false, content_matches: Vec::new(), content_matches_truncated: false,
                    follow_on: Vec::new(),
                }, BTreeSet::new()));
                if semantic_oids.contains(hunk.oid.as_str())
                    && !suggestion.selection_routes.contains(&"local_semantic")
                {
                    suggestion.selection_routes.push("local_semantic");
                }
                bases.insert(index);
                if !suggestion.associated_current_paths.contains(&local.path) {
                    suggestion.associated_current_paths.push(local.path.clone());
                }
                if suggestion.content_matches.len() >= MATCH_LIMIT {
                    suggestion.content_matches_truncated = true;
                    continue;
                }
                suggestion.content_matches.push(ContentMatch {
                    current_path: local.path.clone(),
                    current_added: local.added,
                    current_old_start: local.old_start,
                    current_new_start: local.new_start,
                    historical_oid: hunk.oid.clone(),
                    historical_path: path.clone(),
                    historical_added: added,
                    historical_line: line_number,
                    historical_old_start: hunk.old_start,
                    historical_new_start: hunk.new_start,
                    signals: shared,
                    excerpt: text[..text.len().min(EXCERPT_LIMIT)].to_vec(),
                    excerpt_truncated: text.len() > EXCERPT_LIMIT,
                });
            }
        }
        Ok(())
    };
    let (truncated, oversized) =
        session.scan_code_hunks_bounded(scope, |hunk| verify(hunk, CANDIDATE_LIMIT))?;
    // Targeted bounded patches can add commits beyond the ordinary scan window.
    // The same verifier and identities handle both routes before output selection.
    for candidate in &semantic {
        let history =
            session.patch_history(&candidate.oid, SEMANTIC_HUNK_LIMIT, SEMANTIC_HUNK_BYTES)?;
        report.semantic_content_truncated |= history.truncated || history.missing_objects;
        for hunk in history.hunks {
            let Some(text) = hunk.text else {
                continue;
            };
            verify(
                crate::cache::CodeHunk {
                    oid: candidate.oid.clone(),
                    commit_time: candidate.commit_time,
                    change_ordinal: hunk.change_ordinal,
                    hunk_ordinal: hunk.hunk_ordinal,
                    old_path: hunk.old_path,
                    new_path: hunk.new_path,
                    old_start: hunk.old_start,
                    new_start: hunk.new_start,
                    text,
                },
                CANDIDATE_LIMIT + SEMANTIC_CANDIDATE_LIMIT,
            )?;
        }
    }
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
