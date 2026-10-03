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
}

impl Context {
    pub(in crate::analysis) fn open(options: SearchScopeOptions) -> Result<Self, AppError> {
        let repository = Repository::discover()?;
        let session = cache::open_query(&repository.root)?;
        let scope = scope::resolve(&session, options)?;
        Ok(Self { session, scope })
    }

    pub(in crate::analysis) fn for_target(
        session: QuerySession,
        options: SearchScopeOptions,
        revision: &str,
    ) -> Result<Self, AppError> {
        let scope = scope::resolve_for_target(&session, options, revision)?;
        Ok(Self { session, scope })
    }

    pub(in crate::analysis) fn for_head_target(
        session: QuerySession,
        options: SearchScopeOptions,
        revision: &str,
    ) -> Result<Self, AppError> {
        let scope = scope::resolve_for_head_target(&session, options, revision)?;
        Ok(Self { session, scope })
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
                "incomplete history coverage for {}: results include only cached reachable commits; run `gitscry index` to index more history",
                scope.report.to_rev
            ));
        }
        let scope = self.scope.map(|scope| scope.report);
        match &mut report {
            QueryReport::Followups(_) => {}
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
