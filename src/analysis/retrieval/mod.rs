//! Reading and scoring candidates out of the completed cache generation.
//!
//! One reason to change: how cached history is interpreted and scored.
//! Capabilities share candidate retrieval, ranking, and line-history interpretation here;
//! storage stays in `cache`, and report wording stays in `capabilities`.

mod intent;
mod lexical;
mod line_history;
mod rank;
mod reverts;
mod text;

use super::Step;
use crate::{app::AppError, cache::QuerySession};

pub(crate) use crate::cache::message_parts;
pub(in crate::analysis) use crate::cache::{HistoryCommit, HistoryHunk, RelationHistory};
pub(crate) use intent::Intent;
pub(in crate::analysis) use lexical::Signals;
pub(in crate::analysis) use line_history::{hunk_overlaps_symbol, trace_line};
pub(in crate::analysis) use rank::{Ranked, assign_citations, sort};
pub(in crate::analysis) use reverts::{Revert, RevertIndex, index as reverts};
pub(in crate::analysis) use text::tokenize;
pub(crate) use text::{normalize_path, searchable_text};
/// How deep the lexical pool reaches relative to the caller's `--limit`.
const CANDIDATE_MULTIPLIER: usize = 20;

/// One candidate with everything retrieval knows about it.
pub(in crate::analysis) struct Scored {
    /// Position in the cache generation, which orders commits recorded in the same second.
    pub(in crate::analysis) position: i64,
    pub(in crate::analysis) oid: String,
    pub(in crate::analysis) commit_time: i64,
    pub(in crate::analysis) subject: String,
    pub(in crate::analysis) paths: Vec<Vec<u8>>,
    pub(in crate::analysis) path_keys: Vec<String>,
    pub(in crate::analysis) signals: Signals,
}

/// Every candidate an intent reaches, with the total number of matching commits.
pub(in crate::analysis) struct Pool {
    pub(in crate::analysis) matched_count: usize,
    pub(in crate::analysis) candidates: Vec<Scored>,
}

impl Pool {
    /// How many of another candidate's paths this one also changed.
    pub(in crate::analysis) fn shared_paths(&self, index: usize, other: usize) -> usize {
        let (Some(left), Some(right)) = (self.candidates.get(index), self.candidates.get(other))
        else {
            return 0;
        };
        left.paths
            .iter()
            .filter(|path| right.paths.iter().any(|other| other == *path))
            .count()
    }
}

pub(in crate::analysis) fn pool(
    session: &QuerySession,
    intent: &Intent,
    limit: usize,
) -> Result<Option<Pool>, AppError> {
    let terms = intent.terms();
    if terms.is_empty() {
        return Ok(None);
    }
    let query = match_query(terms);
    let matched_count = session.match_count(&query)?;
    if matched_count == 0 {
        return Ok(None);
    }

    let candidates = session
        .candidates(&query, candidate_limit(limit))?
        .into_iter()
        .map(|candidate| Scored {
            signals: lexical::signals(
                intent,
                &candidate.subject,
                &candidate.body,
                &candidate.paths,
                candidate.bm25,
            ),
            position: candidate.position,
            oid: candidate.oid,
            commit_time: candidate.commit_time,
            subject: candidate.subject,
            path_keys: candidate.path_keys,
            paths: candidate.paths,
        })
        .collect();
    Ok(Some(Pool {
        matched_count,
        candidates,
    }))
}

pub(in crate::analysis) fn steps(session: &QuerySession, oid: &str) -> Result<Vec<Step>, AppError> {
    let mut steps = Vec::new();
    for change in session.changes(oid)? {
        if let Some(step) = Step::from_change(&change.status, change.old_path, change.new_path)
            && !steps.contains(&step)
        {
            steps.push(step);
        }
    }
    Ok(steps)
}

pub(in crate::analysis) fn commit_text(
    session: &QuerySession,
    oid: &str,
) -> Result<Option<(String, String)>, AppError> {
    Ok(session
        .commit_message(oid)?
        .map(|message| message_parts(&message)))
}

pub(in crate::analysis) fn corrective_follow_up(
    session: &QuerySession,
    revert_oid: &str,
    path_keys: &[String],
) -> Result<Option<(String, String, String)>, AppError> {
    for commit in session.follow_ups(revert_oid, path_keys)? {
        let (subject, body) = message_parts(&commit.message);
        if super::provenance::is_corrective_subject(&subject) {
            return Ok(Some((commit.oid, subject, body)));
        }
    }
    Ok(None)
}

/// The revert history associates with a candidate: the trailer it names first, then the
/// earliest later revert of the same paths that no intervening commit also touched.
pub(in crate::analysis) fn link<'a>(
    session: &QuerySession,
    reverts: &'a RevertIndex,
    candidate: &Scored,
) -> Result<Option<&'a Revert>, AppError> {
    if let Some(revert) = reverts.of(&candidate.oid) {
        return Ok(Some(revert));
    }
    for revert in reverts.undoings(&candidate.oid, &candidate.path_keys, candidate.position) {
        if !session.touched_between(candidate.position, revert.position, &candidate.path_keys)? {
            return Ok(Some(revert));
        }
    }
    Ok(None)
}

fn match_query(terms: &[String]) -> String {
    terms
        .iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn candidate_limit(limit: usize) -> i64 {
    i64::try_from(limit.saturating_mul(CANDIDATE_MULTIPLIER)).unwrap_or(i64::MAX)
}
