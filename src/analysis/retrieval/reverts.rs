use std::collections::{HashMap, HashSet};

use rusqlite::Connection;

use crate::app::AppError;

use super::super::provenance::{is_revert_subject, reverted_commit};
use super::store;
use super::text::message_parts;

/// A cached commit whose subject reads like a revert, with what is needed to link it to the
/// work it undid. The `This reverts commit` trailer is authoritative; most reverts in the
/// wild omit it, so normalized path keys are recorded as the fallback link.
pub(in crate::analysis) struct Revert {
    pub(in crate::analysis) position: i64,
    pub(in crate::analysis) oid: String,
    pub(in crate::analysis) subject: String,
    pub(in crate::analysis) body: String,
    pub(in crate::analysis) path_keys: Vec<String>,
    target: Option<String>,
}

/// Every cached revert, indexed by the commit its trailer names.
pub(in crate::analysis) struct RevertIndex {
    reverts: Vec<Revert>,
    by_target: HashMap<String, usize>,
}

impl RevertIndex {
    /// Whether `oid` is itself a recorded revert.
    pub(in crate::analysis) fn is_revert(&self, oid: &str) -> bool {
        self.reverts.iter().any(|revert| revert.oid == oid)
    }

    /// The revert this commit's own message names, when the trailer is present.
    pub(in crate::analysis) fn of(&self, oid: &str) -> Option<&Revert> {
        self.by_target.get(oid).map(|index| &self.reverts[*index])
    }

    /// Later reverts that undid every path this commit changed, earliest first.
    ///
    /// An undo touches everything the undone change touched, so requiring the revert to
    /// cover all of the candidate's paths rejects a revert that merely shares a file with
    /// unrelated work.
    pub(in crate::analysis) fn undoings<'a>(
        &'a self,
        oid: &str,
        path_keys: &[String],
        position: i64,
    ) -> impl Iterator<Item = &'a Revert> {
        self.reverts
            .iter()
            .filter(move |revert| {
                revert.oid != oid
                    && revert.position > position
                    && !path_keys.is_empty()
                    && path_keys.iter().all(|key| revert.path_keys.contains(key))
            })
            .collect::<Vec<_>>()
            .into_iter()
    }

    /// The revert that names a cached commit in its trailer, when `oid` is that revert.
    pub(in crate::analysis) fn resolved_revert(&self, oid: &str) -> Option<&Revert> {
        self.reverts
            .iter()
            .find(|revert| revert.oid == oid && revert.target.is_some())
    }

    /// The cached commit this subject's trailer names, when it names one.
    pub(in crate::analysis) fn target_of(&self, oid: &str) -> Option<&str> {
        self.reverts
            .iter()
            .find(|revert| revert.oid == oid)
            .and_then(|revert| revert.target.as_deref())
    }
}

/// Index every cached revert in cache order; the earliest resolved revert of a commit wins.
pub(in crate::analysis) fn index(connection: &Connection) -> Result<RevertIndex, AppError> {
    let commits = store::history(connection)?;
    let known = commits
        .iter()
        .map(|commit| commit.oid.clone())
        .collect::<HashSet<_>>();
    let mut reverts = Vec::new();
    for commit in commits {
        let (subject, body) = message_parts(&commit.message);
        if !is_revert_subject(&subject) {
            continue;
        }
        let target =
            reverted_commit(&format!("{subject}\n{body}")).and_then(|hex| resolve(&known, &hex));
        reverts.push(Revert {
            path_keys: store::projected_path_keys(connection, &commit.oid)?,
            oid: commit.oid,
            subject,
            body,
            position: commit.position,
            target,
        });
    }
    let mut by_target = HashMap::new();
    for (position, revert) in reverts.iter().enumerate() {
        if let Some(target) = &revert.target {
            by_target.entry(target.clone()).or_insert(position);
        }
    }
    Ok(RevertIndex { reverts, by_target })
}

/// Resolve an abbreviated object ID, but only when it is unambiguous.
fn resolve(known: &HashSet<String>, hex: &str) -> Option<String> {
    if hex.len() < 7 {
        return None;
    }
    let mut matches = known.iter().filter(|oid| oid.starts_with(hex));
    let resolved = matches.next()?.clone();
    matches.next().is_none().then_some(resolved)
}
