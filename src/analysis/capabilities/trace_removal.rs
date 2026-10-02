//! Text-matched deletion events, not inferred code lifecycles.
use std::collections::BTreeMap;

use crate::{
    analysis::{CodeDirection, CodeMatch, SearchScopeInfo},
    app::AppError,
    cache::{QuerySession, SearchFilter},
};

pub(crate) struct Report {
    pub(crate) query: String,
    pub(crate) path: Option<Vec<u8>>,
    pub(crate) cache_tip: String,
    pub(crate) scope: Option<SearchScopeInfo>,
    pub(crate) limit: usize,
    pub(crate) matched_count: usize,
    pub(crate) truncated: bool,
    pub(crate) events: Vec<Event>,
}

pub(crate) struct Event {
    pub(crate) commit_id: String,
    pub(crate) timestamp: String,
    pub(crate) message: Vec<u8>,
    pub(crate) first_parent_id: String,
    pub(crate) old_path: Vec<u8>,
    pub(crate) status: String,
    pub(crate) new_path: Option<Vec<u8>>,
    pub(crate) matches: Vec<CodeMatch>,
}

pub(crate) fn run(
    session: &QuerySession,
    query: &str,
    path: Option<&str>,
    limit: usize,
    scope: Option<&SearchFilter>,
) -> Result<Report, AppError> {
    // The line-search limit must never cut an event's matching positions.
    let lines = match scope {
        Some(scope) => super::code_search::run_scoped(
            session,
            query,
            path,
            Some(CodeDirection::Removed),
            usize::MAX,
            scope,
        )?,
        None => super::code_search::run(
            session,
            query,
            path,
            Some(CodeDirection::Removed),
            usize::MAX,
        )?,
    };
    let mut groups: BTreeMap<(String, Vec<u8>), Vec<CodeMatch>> = BTreeMap::new();
    for line in lines.code_matches {
        groups
            .entry((line.commit_id.clone(), line.path.clone()))
            .or_default()
            .push(line);
    }
    let mut events = Vec::new();
    for ((commit_id, old_path), mut matches) in groups {
        let change = session.removal_change(&commit_id, &old_path)?;
        let Some(first_parent_id) = change.first_parent_id else {
            continue;
        };
        matches.sort_by(|a, b| {
            a.line_number
                .cmp(&b.line_number)
                .then_with(|| a.line.cmp(&b.line))
        });
        matches.dedup_by(|a, b| a.line_number == b.line_number && a.line == b.line);
        events.push((
            change.commit_time,
            Event {
                commit_id,
                timestamp: super::timeline::format_timestamp(change.commit_time),
                message: change.message,
                first_parent_id,
                old_path,
                status: change.status,
                new_path: change.new_path,
                matches,
            },
        ));
    }
    events.sort_by(|(a_time, a), (b_time, b)| {
        b_time
            .cmp(a_time)
            .then_with(|| a.commit_id.cmp(&b.commit_id))
            .then_with(|| a.old_path.cmp(&b.old_path))
    });
    let matched_count = events.len();
    Ok(Report {
        query: query.to_owned(),
        path: path.map(|path| path.as_bytes().to_vec()),
        cache_tip: session.completed_tip()?,
        scope: None,
        limit,
        matched_count,
        truncated: matched_count > limit,
        events: events
            .into_iter()
            .take(limit)
            .map(|(_, event)| event)
            .collect(),
    })
}
