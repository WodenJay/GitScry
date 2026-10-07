//! The published generation and scope shared by capability execution.

use std::collections::HashSet;

use super::{Outcome, QueryReport, SearchScopeOptions, scope};
use crate::{
    app::AppError,
    cache::{self, QuerySession, SearchFilter},
    git::Repository,
};

/// The cache generation and scope shared by material selection and excerpts.
pub(in crate::analysis) struct Context {
    pub(in crate::analysis) session: QuerySession,
    pub(in crate::analysis) scope: Option<scope::ResolvedSearchScope>,
    pub(in crate::analysis) pinned_head: String,
}
/// Cache prerequisites of pinned Git targets, not material-selection policy.
pub(in crate::analysis) trait QueryTarget {
    fn revision(&self) -> &str;

    fn additional_revision(&self) -> Option<&str> {
        None
    }
}

impl QueryTarget for String {
    fn revision(&self) -> &str {
        self
    }
}

impl QueryTarget for crate::git::WhyTarget {
    fn revision(&self) -> &str {
        &self.revision
    }
}

impl QueryTarget for crate::git::TimelineTarget {
    fn revision(&self) -> &str {
        &self.revision
    }
}

impl QueryTarget for crate::git::FateTarget {
    fn revision(&self) -> &str {
        &self.revision
    }
}

impl QueryTarget for crate::git::TraceFixTarget {
    fn revision(&self) -> &str {
        &self.revision
    }
}

impl QueryTarget for crate::git::RegressionTarget {
    fn revision(&self) -> &str {
        &self.bad_revision
    }
    fn additional_revision(&self) -> Option<&str> {
        self.good_revision.as_deref()
    }
}

impl Context {
    pub(in crate::analysis) fn open(options: SearchScopeOptions) -> Result<Self, AppError> {
        let repository = Repository::discover()?;
        let head = Self::pin_current_head(&repository)?;
        Self::open_pinned(&repository, head, options)
    }

    /// Refresh and query the history target already pinned by the capability.
    pub(in crate::analysis) fn open_pinned(
        repository: &Repository,
        head: String,
        options: SearchScopeOptions,
    ) -> Result<Self, AppError> {
        let session = cache::refresh_query(repository, &head)?;
        let scope = scope::resolve_for_query(&session, options, &head)?;
        Ok(Self {
            session,
            scope,
            pinned_head: head,
        })
    }

    /// Pin HEAD while preserving the initialized-cache prerequisite on failure.
    pub(in crate::analysis) fn pin_current_head(
        repository: &Repository,
    ) -> Result<String, AppError> {
        match repository.resolve_commit("HEAD") {
            Ok(head) => Ok(head),
            Err(error) => {
                cache::open_query(repository)?;
                Err(error)
            }
        }
    }

    /// Pin the feature target before waiting for refresh, then validate its
    /// cache prerequisites and install scope. An implicit HEAD target may use
    /// safely cached history even when refresh could not publish HEAD itself.
    pub(in crate::analysis) fn prepare_target<T: QueryTarget>(
        repository: &Repository,
        requested: Option<&str>,
        options: SearchScopeOptions,
        pin: impl FnOnce(&str) -> Result<T, AppError>,
    ) -> Result<(Self, T), AppError> {
        let head = Self::pin_current_head(repository)?;
        let target = pin(requested.unwrap_or(&head))?;
        let session = cache::refresh_query(repository, &head)?;
        if requested.is_some() {
            session.require_revision(target.revision())?;
        }
        if let Some(revision) = target.additional_revision() {
            session.require_revision(revision)?;
        }
        let scope = if requested.is_none() {
            scope::resolve_for_head_target(&session, options, target.revision())?
        } else {
            scope::resolve_for_target(&session, options, target.revision())?
        };
        Ok((
            Self {
                session,
                scope,
                pinned_head: head,
            },
            target,
        ))
    }

    pub(in crate::analysis) fn filter(&self) -> Option<&SearchFilter> {
        self.scope.as_ref().map(|scope| &scope.filter)
    }

    /// Eligibility is distinct from traversal: why and timeline must follow
    /// the complete target history before filtering material or pagination.
    pub(in crate::analysis) fn eligible_revisions(
        &self,
        revision: &str,
    ) -> Result<Option<HashSet<String>>, AppError> {
        self.filter()
            .map(|filter| self.session.scoped_revisions(filter, revision))
            .transpose()
    }

    pub(in crate::analysis) fn intersect(
        &self,
        revision: &str,
        reachable: &mut HashSet<String>,
    ) -> Result<(), AppError> {
        if let Some(eligible) = self.eligible_revisions(revision)? {
            reachable.retain(|revision| eligible.contains(revision));
        }
        Ok(())
    }

    pub(in crate::analysis) fn finish(self, mut report: QueryReport) -> Outcome {
        let mut warnings = self.session.warnings().to_vec();
        if let Some(scope) = &self.scope
            && !scope.report.coverage_complete
        {
            warnings.push(format!(
                "incomplete history coverage for {}: results include only reachable commits available in the published cache; run `gitscry index` after making more local history available",
                scope.report.to_rev
            ));
        }
        let scope = self.scope.map(|scope| scope.report);
        match &mut report {
            QueryReport::Conflicts(_) | QueryReport::Followups(_) => {}
            QueryReport::PatchSearch(_) => {}
            QueryReport::Patterns(report) => report.scope = scope,
            QueryReport::Context(report) => report.scope = scope,
            QueryReport::Analysis(report) => report.scope = scope,
            QueryReport::FragmentSearch(report) => report.scope = scope,
            QueryReport::Timeline(report) => report.scope = scope,
            QueryReport::TraceRemoval(report) => report.scope = scope,
            QueryReport::TraceRemovalFragment(report) => report.scope = scope,
            QueryReport::Fate(report) => report.scope = scope,
            QueryReport::Hotspots(report) => report.scope = scope,
            QueryReport::Propagation(_) => {}
        }
        Outcome {
            progress: self.session.progress().to_vec(),
            warnings,
            report,
        }
    }
}
