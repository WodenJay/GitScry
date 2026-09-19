use std::collections::{HashMap, HashSet};

use rusqlite::Connection;

use crate::app::AppError;

use super::super::provenance::{is_revert_subject, reverted_commit};
use super::store;
use super::text::message_parts;

/// A cached commit whose subject reads like a revert, with the commit it reverts when known.
pub(in crate::analysis) struct Revert {
    pub(in crate::analysis) oid: String,
    pub(in crate::analysis) subject: String,
    pub(in crate::analysis) body: String,
    pub(in crate::analysis) target: Option<String>,
}

/// Every cached revert marker, indexed by the commit it reverts.
pub(in crate::analysis) struct RevertIndex {
    reverts: Vec<Revert>,
    by_target: HashMap<String, usize>,
}

impl RevertIndex {
    /// The earliest revert history records for `oid`.
    pub(in crate::analysis) fn reverting(&self, oid: &str) -> Option<&Revert> {
        self.by_target.get(oid).map(|index| &self.reverts[*index])
    }
}

/// Index every cached revert marker; the earliest recorded revert of a commit wins.
pub(in crate::analysis) fn index(connection: &Connection) -> Result<RevertIndex, AppError> {
    let commits = store::all_texts(connection)?;
    let known = commits
        .iter()
        .map(|(oid, _)| oid.clone())
        .collect::<HashSet<_>>();
    let mut reverts = Vec::new();
    for (oid, message) in commits {
        let (subject, body) = message_parts(&message);
        if !is_revert_subject(&subject) {
            continue;
        }
        let target =
            reverted_commit(&format!("{subject}\n{body}")).and_then(|hex| resolve(&known, &hex));
        reverts.push(Revert {
            oid,
            subject,
            body,
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
