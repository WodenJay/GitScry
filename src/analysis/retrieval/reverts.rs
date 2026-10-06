use std::collections::{HashMap, HashSet};

use crate::{
    app::AppError,
    cache::{QuerySession, SearchFilter},
};

use super::super::provenance::{explicit_revert_declarations, is_revert_subject};
use super::message_parts;

/// A cached commit whose message explicitly declares what it reverts, or whose subject
/// reads like a revert. The declaration is authoritative; a subject alone never names a
/// target, so revert-like commits without a declaration carry no revert relationship.
pub(in crate::analysis) struct Revert {
    pub(in crate::analysis) oid: String,
    pub(in crate::analysis) subject: String,
    pub(in crate::analysis) body: String,
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

    /// The revert that names a cached commit in its message, when `oid` is that revert.
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

/// Index every cached commit that reads like a revert or explicitly declares one, in cache
/// order; the earliest resolved revert of a commit wins. A message declaring two different
/// targets records no target at all: the declaration contract is single-target.
pub(in crate::analysis) fn index(
    session: &QuerySession,
    scope: Option<&SearchFilter>,
) -> Result<RevertIndex, AppError> {
    let commits = match scope {
        Some(scope) => session.commits_scoped(scope)?,
        None => session.commits()?,
    };
    let eligible = commits
        .iter()
        .map(|commit| commit.oid.clone())
        .collect::<HashSet<_>>();
    let all_cached = match scope {
        Some(_) => session.commit_oids()?,
        None => eligible.clone(),
    };
    let mut reverts = Vec::new();
    for commit in commits {
        let (subject, body) = message_parts(&commit.message);
        let single_target = explicit_revert_declarations(&format!("{subject}\n{body}"))
            .into_iter()
            .map(|hex| resolve_in_scope(&all_cached, &eligible, &hex))
            .collect::<Option<Vec<_>>>()
            .and_then(|targets| {
                let unique = targets.into_iter().collect::<HashSet<_>>();
                (unique.len() == 1)
                    .then(|| unique.into_iter().next())
                    .flatten()
            });
        if !is_revert_subject(&subject) && single_target.is_none() {
            continue;
        }
        reverts.push(Revert {
            oid: commit.oid,
            subject,
            body,
            target: single_target,
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
pub(in crate::analysis) fn resolve_oid_prefix(
    known: &HashSet<String>,
    hex: &str,
) -> Option<String> {
    if hex.len() < 7 {
        return None;
    }
    let mut matches = known.iter().filter(|oid| oid.starts_with(hex));
    let resolved = matches.next()?.clone();
    matches.next().is_none().then_some(resolved)
}

fn resolve_in_scope(
    all_cached: &HashSet<String>,
    eligible: &HashSet<String>,
    hex: &str,
) -> Option<String> {
    resolve_oid_prefix(all_cached, hex).filter(|target| eligible.contains(target))
}

#[cfg(test)]
mod tests {
    use super::resolve_in_scope;
    use std::collections::HashSet;

    #[test]
    fn revert_target_prefix_must_be_unique_across_all_cached_commits() {
        let all_cached = HashSet::from([
            "1234567aaaa".to_owned(),
            "1234567bbbb".to_owned(),
            "abcdef01234".to_owned(),
            "fedcba01234".to_owned(),
        ]);
        let eligible = HashSet::from(["1234567aaaa".to_owned(), "abcdef01234".to_owned()]);

        assert_eq!(resolve_in_scope(&all_cached, &eligible, "1234567"), None);
        assert_eq!(
            resolve_in_scope(&all_cached, &eligible, "abcdef0"),
            Some("abcdef01234".to_owned())
        );
        assert_eq!(resolve_in_scope(&all_cached, &eligible, "fedcba0"), None);
    }
}
