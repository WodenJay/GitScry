//! Reading and scoring candidates out of the completed cache generation.
//!
//! One reason to change: how history is read and how strongly it answers an intent.
//! Capabilities cross this seam through [`Pool`] and [`reverts`]; everything else here
//! stays private.

mod intent;
mod lexical;
mod rank;
mod reverts;
mod store;
mod text;

use rusqlite::Connection;

use crate::app::AppError;

pub(crate) use intent::Intent;
pub(crate) use text::{message_parts, normalize_path, searchable_text};

pub(in crate::analysis) use lexical::Signals;
pub(in crate::analysis) use rank::{Ranked, assign_citations, sort};
pub(in crate::analysis) use reverts::{Revert, RevertIndex, index as reverts};
pub(in crate::analysis) use store::{
    ChangeSet, change_sets, corrective_follow_up, steps, text as commit_text,
};
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

/// Read the candidates an intent reaches, or `None` when it reaches none.
pub(in crate::analysis) fn pool(
    connection: &Connection,
    intent: &Intent,
    limit: usize,
) -> Result<Option<Pool>, AppError> {
    let terms = intent.terms();
    if terms.is_empty() {
        return Ok(None);
    }
    let query = match_query(terms);
    let matched_count = store::match_count(connection, &query)?;
    if matched_count == 0 {
        return Ok(None);
    }

    let candidates = store::candidates(connection, &query, candidate_limit(limit))?
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
            paths: candidate.paths,
        })
        .collect();
    Ok(Some(Pool {
        matched_count,
        candidates,
    }))
}

/// The revert history associates with a candidate: the trailer it names first, then the
/// earliest later revert of the same paths that no intervening commit also touched.
pub(in crate::analysis) fn link<'a>(
    connection: &Connection,
    reverts: &'a RevertIndex,
    candidate: &Scored,
) -> Result<Option<&'a Revert>, AppError> {
    if let Some(revert) = reverts.of(&candidate.oid) {
        return Ok(Some(revert));
    }
    for revert in reverts.undoings(&candidate.oid, &candidate.paths, candidate.position) {
        if !store::touch_between(
            connection,
            candidate.position,
            revert.position,
            &candidate.paths,
        )? {
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
