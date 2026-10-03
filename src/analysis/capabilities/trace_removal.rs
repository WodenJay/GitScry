//! Text-matched deletion events, not inferred code lifecycles.
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
};

use crate::{
    analysis::{CodeDirection, CodeMatch, PatchExcerpt, SearchScopeInfo},
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
    pub(crate) change_ordinal: i64,
    pub(crate) patch: Option<PatchExcerpt>,
}

// Only event identities are retained during discovery, never omitted source lines.
struct EventSelection {
    limit: usize,
    seen: BTreeSet<(String, Vec<u8>)>,
    selected: BTreeSet<(Reverse<i64>, String, Vec<u8>)>,
}

impl EventSelection {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            seen: BTreeSet::new(),
            selected: BTreeSet::new(),
        }
    }

    fn record(&mut self, time: i64, oid: &str, path: &[u8]) {
        if self.seen.insert((oid.to_owned(), path.to_vec())) {
            self.selected
                .insert((Reverse(time), oid.to_owned(), path.to_vec()));
            if self.selected.len() > self.limit {
                self.selected.pop_last();
            }
        }
    }
}

pub(crate) fn run(
    session: &QuerySession,
    query: &str,
    path: Option<&str>,
    limit: usize,
    scope: Option<&SearchFilter>,
) -> Result<Report, AppError> {
    let mut selection = EventSelection::new(limit);
    super::code_search::visit_matches(
        session,
        query,
        path,
        CodeDirection::Removed,
        scope,
        |hunk, _, _| {
            let old_path = hunk
                .old_path
                .as_deref()
                .expect("removed line has an old path");
            let key = (hunk.oid.clone(), old_path.to_vec());
            if !selection.seen.contains(&key)
                && session
                    .removal_change(&hunk.oid, old_path)?
                    .first_parent_id
                    .is_some()
            {
                selection.record(hunk.commit_time, &hunk.oid, old_path);
            }
            Ok(())
        },
    )?;
    let matched_count = selection.seen.len();
    let mut groups: BTreeMap<_, Vec<CodeMatch>> = selection
        .selected
        .iter()
        .map(|(_, oid, path)| ((oid.clone(), path.clone()), Vec::new()))
        .collect();
    drop(selection);
    // A second pass retains complete line evidence only for the selected events.
    if !groups.is_empty() {
        super::code_search::visit_matches(
            session,
            query,
            path,
            CodeDirection::Removed,
            scope,
            |hunk, line_number, line| {
                let old_path = hunk
                    .old_path
                    .as_deref()
                    .expect("removed line has an old path");
                if let Some(matches) = groups.get_mut(&(hunk.oid.clone(), old_path.to_vec())) {
                    matches.push(CodeMatch {
                        commit_id: hunk.oid.clone(),
                        path: old_path.to_vec(),
                        direction: CodeDirection::Removed,
                        line_number,
                        line: line.to_vec(),
                    });
                }
                Ok(())
            },
        )?;
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
                change_ordinal: change.change_ordinal,
                patch: None,
            },
        ));
    }
    events.sort_by(|(a_time, a), (b_time, b)| {
        b_time
            .cmp(a_time)
            .then_with(|| a.commit_id.cmp(&b.commit_id))
            .then_with(|| a.old_path.cmp(&b.old_path))
    });
    Ok(Report {
        query: query.to_owned(),
        path: path.map(|path| path.as_bytes().to_vec()),
        cache_tip: session.completed_tip()?,
        scope: None,
        limit,
        matched_count,
        truncated: matched_count > limit,
        events: events.into_iter().map(|(_, event)| event).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::EventSelection;

    #[test]
    fn selection_counts_all_events_but_retains_only_the_limit_and_no_line_payloads() {
        let mut selection = EventSelection::new(2);
        for time in 0..100 {
            for _ in 0..50 {
                selection.record(time, &time.to_string(), b"a.rs");
            }
            assert!(selection.selected.len() <= 2);
        }
        assert_eq!(selection.seen.len(), 100);
        let ids: Vec<_> = selection
            .selected
            .iter()
            .map(|(_, oid, _)| oid.as_str())
            .collect();
        assert_eq!(ids, ["99", "98"]);
    }
}
