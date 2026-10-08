//! Attach recorded provenance only to changes already associated with current content.
use super::{Category, Suggestion};
use crate::{
    analysis::{
        Citation, Failure,
        provenance::{revert_reason, stated_retry},
        retrieval,
    },
    app::AppError,
    cache::{QuerySession, SearchFilter},
};
use std::collections::BTreeMap;

const MATCH_LIMIT: usize = 16;

pub(super) fn compose(
    session: &QuerySession,
    changes: Vec<(usize, i64, Suggestion)>,
    scope: Option<&SearchFilter>,
) -> Result<Vec<(usize, i64, Suggestion)>, AppError> {
    if changes.is_empty() {
        return Ok(changes);
    }
    let reverts = retrieval::reverts(session, scope)?;
    let mut recordings = BTreeMap::new();
    let mut originals_by_revert: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (_, _, suggestion) in &changes {
        let oid = suggestion.citations[0].oid.as_str();
        if let Some(revert) = retrieval::link_change(&reverts, oid) {
            recordings.insert(oid.to_owned(), revert);
            originals_by_revert
                .entry(&revert.oid)
                .or_default()
                .push(oid.to_owned());
        }
    }
    let mut grouped: BTreeMap<String, (i64, Suggestion)> = BTreeMap::new();
    for (_, time, mut suggestion) in changes {
        let oid = suggestion.citations[0].oid.clone();
        let original = reverts
            .target_of(&oid)
            .or_else(|| {
                originals_by_revert
                    .get(oid.as_str())
                    .and_then(|originals| (originals.len() == 1).then_some(originals[0].as_str()))
            })
            .unwrap_or(&oid)
            .to_owned();
        if let Some((latest, existing)) = grouped.get_mut(&original) {
            *latest = (*latest).max(time);
            for path in suggestion.associated_current_paths {
                if !existing.associated_current_paths.contains(&path) {
                    existing.associated_current_paths.push(path);
                }
            }
            for basis in suggestion.basis {
                if !existing.basis.contains(&basis) {
                    existing.basis.push(basis);
                }
            }
            existing.content_matches_truncated |= suggestion.content_matches_truncated;
            for matched in suggestion.content_matches {
                if existing.content_matches.len() == MATCH_LIMIT {
                    existing.content_matches_truncated = true;
                    break;
                }
                existing.content_matches.push(matched);
            }
        } else {
            // Canonical identity is the underlying original, not the matching route.
            suggestion.citations = vec![Citation::new(
                original.clone(),
                retrieval::commit_text(session, &original)?
                    .map(|(subject, _)| subject)
                    .unwrap_or_default(),
            )];
            grouped.insert(original, (time, suggestion));
        }
    }
    let mut found = Vec::new();
    for (oid, (time, mut suggestion)) in grouped {
        let recording = recordings
            .get(oid.as_str())
            .copied()
            .or_else(|| reverts.of(&oid));
        if let Some(revert) = recording {
            suggestion.category = Category::RecordedAbandonment;
            suggestion.basis.push("recorded revert of the associated underlying change; provenance is not an additional relevance signal".to_owned());
            suggestion.selection_routes.push("recorded_revert");
            suggestion.citations.push(
                Citation::new(revert.oid.clone(), revert.subject.clone())
                    .noting("reverts this change"),
            );
            let reason = revert_reason(&revert.subject, &revert.body);
            let mut retry = stated_retry(&revert.body);
            let paths = session.projected_paths(&oid)?;
            if let Some((follow_oid, subject, body)) =
                retrieval::corrective_follow_up(session, scope, &revert.oid, &paths)?
            {
                retry = stated_retry(&format!("{subject}\n{body}")).or(retry);
                suggestion
                    .citations
                    .push(Citation::new(follow_oid, subject).noting("follow-up"));
            }
            suggestion.abandonment = Some(Failure {
                reason,
                retry,
                patch_equivalence: None,
            });
        }
        suggestion.associated_current_paths.sort();
        // Tie time comes only from verified associations, never attached provenance.
        found.push((suggestion.associated_current_paths.len(), time, suggestion));
    }
    Ok(found)
}
