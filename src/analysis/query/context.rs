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

impl Context {
    pub(in crate::analysis) fn open(options: SearchScopeOptions) -> Result<Self, AppError> {
        let repository = Repository::discover()?;
        let head = Self::pin_current_head(&repository)?;
        let session = Self::refresh_query(&repository, &head)?;
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

    /// Refresh the initialized cache against this invocation's pinned HEAD.
    pub(in crate::analysis) fn refresh_query(
        repository: &Repository,
        pinned_head: &str,
    ) -> Result<QuerySession, AppError> {
        cache::refresh_query(repository, pinned_head)
    }

    pub(in crate::analysis) fn for_target(
        session: QuerySession,
        options: SearchScopeOptions,
        revision: &str,
        pinned_head: &str,
    ) -> Result<Self, AppError> {
        let scope = scope::resolve_for_target(&session, options, revision)?;
        Ok(Self {
            session,
            scope,
            pinned_head: pinned_head.to_owned(),
        })
    }

    pub(in crate::analysis) fn for_head_target(
        session: QuerySession,
        options: SearchScopeOptions,
        revision: &str,
        pinned_head: &str,
    ) -> Result<Self, AppError> {
        let scope = scope::resolve_for_head_target(&session, options, revision)?;
        Ok(Self {
            session,
            scope,
            pinned_head: pinned_head.to_owned(),
        })
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
            QueryReport::Patterns(report) => report.scope = scope,
            QueryReport::Context(report) => report.scope = scope,
            QueryReport::Analysis(report) => report.scope = scope,
            QueryReport::Timeline(report) => report.scope = scope,
            QueryReport::TraceRemoval(report) => report.scope = scope,
            QueryReport::Hotspots(report) => report.scope = scope,
        }
        Outcome {
            progress: self.session.progress().to_vec(),
            warnings,
            report,
        }
    }
}
