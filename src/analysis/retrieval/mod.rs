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
pub(crate) use text::{message_parts, searchable_text};

pub(in crate::analysis) use lexical::Signals;
pub(in crate::analysis) use rank::{Ranked, assign_citations, sort};
pub(in crate::analysis) use reverts::RevertIndex;

use super::Step;

/// How deep the lexical pool reaches relative to the caller's `--limit`.
const CANDIDATE_MULTIPLIER: usize = 20;

/// One candidate with everything retrieval knows about it.
pub(in crate::analysis) struct Scored {
    pub(in crate::analysis) oid: String,
    pub(in crate::analysis) commit_time: i64,
    pub(in crate::analysis) subject: String,
    pub(in crate::analysis) body: String,
    pub(in crate::analysis) paths: Vec<Vec<u8>>,
    pub(in crate::analysis) signals: Signals,
}

/// Every candidate an intent reaches, with the total number of matching commits.
pub(in crate::analysis) struct Pool {
    pub(in crate::analysis) matched_count: usize,
    pub(in crate::analysis) candidates: Vec<Scored>,
}

impl Pool {
    /// The moves each candidate made, in pool order.
    pub(in crate::analysis) fn steps(
        &self,
        connection: &Connection,
    ) -> Result<Vec<Vec<Step>>, AppError> {
        self.candidates
            .iter()
            .map(|candidate| store::steps(connection, &candidate.oid))
            .collect()
    }

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
            signals: lexical::signals(intent, &candidate.subject, &candidate.paths, candidate.bm25),
            oid: candidate.oid,
            commit_time: candidate.commit_time,
            subject: candidate.subject,
            body: candidate.body,
            paths: candidate.paths,
        })
        .collect();
    Ok(Some(Pool {
        matched_count,
        candidates,
    }))
}

/// Resolve every recorded revert in the cache generation.
pub(in crate::analysis) fn reverts(connection: &Connection) -> Result<RevertIndex, AppError> {
    reverts::index(connection)
}

/// Subject and body of one cached commit.
pub(in crate::analysis) fn commit_text(
    connection: &Connection,
    oid: &str,
) -> Result<Option<(String, String)>, AppError> {
    store::text(connection, oid)
}

/// The earliest later commit that touches the abandoned paths.
pub(in crate::analysis) fn corrective_follow_up(
    connection: &Connection,
    after_time: i64,
    paths: &[Vec<u8>],
) -> Result<Option<(String, String)>, AppError> {
    store::corrective_follow_up(connection, after_time, paths)
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
