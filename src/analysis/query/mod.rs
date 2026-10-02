//! Execute a historical query against one published cache generation.
//!
//! Target pinning, scope semantics, material assembly and optional excerpts stay
//! behind this interface. Display formats never participate in query execution.

pub(crate) mod followups;
mod scope;

use std::collections::HashSet;

use super::{CodeDirection, Intent, Report, TimelineReport, capabilities, patch};
use crate::{
    app::AppError,
    cache::{self, QuerySession, SearchFilter},
    git::{Repository, WhyAnchor},
    semantic::Encoder,
};

pub(crate) use scope::SearchScopeOptions;

pub(crate) enum Request {
    Followups {
        revision: String,
        paths: Vec<String>,
        to_rev: Option<String>,
        days: usize,
        max_commits: usize,
    },
    Context {
        staged: bool,
        hybrid: bool,
    },
    Search {
        words: Vec<String>,
        hybrid: bool,
    },
    CodeSearch {
        query: String,
        path: Option<String>,
        direction: Option<CodeDirection>,
    },
    TraceRemoval {
        query: String,
        path: Option<String>,
    },
    Examples {
        words: Vec<String>,
        paths: Vec<String>,
    },
    Failures {
        words: Vec<String>,
        paths: Vec<String>,
    },
    Related(Vec<String>),
    Tests(Vec<String>),
    Regression {
        words: Vec<String>,
        path: String,
        symbol: Option<String>,
        good: Option<String>,
        bad: Option<String>,
    },
    Why {
        revision: Option<String>,
        path: String,
        anchor: WhyAnchor,
    },
    TraceFix {
        revision: String,
        paths: Vec<String>,
    },
    Timeline {
        path: String,
        at: Option<String>,
        offset: usize,
        last: bool,
    },
}

pub(crate) struct Options {
    pub(crate) limit: usize,
    pub(crate) patch: bool,
    pub(crate) scope: SearchScopeOptions,
}

pub(crate) enum QueryReport {
    Followups(followups::Report),
    Context(super::ContextReport),
    Analysis(Report),
    Timeline(TimelineReport),
    TraceRemoval(super::TraceRemovalReport),
}

impl QueryReport {
    pub(crate) fn warnings(&self) -> &[String] {
        match self {
            Self::Followups(report) => &report.warnings,
            Self::Context(report) => &report.warnings,
            Self::Analysis(report) => &report.warnings,
            Self::Timeline(_) => &[],
            Self::TraceRemoval(_) => &[],
        }
    }

    pub(crate) fn notices(&self) -> &[String] {
        match self {
            Self::Followups(_) => &[],
            Self::Context(_) => &[],
            Self::Analysis(report) => &report.notices,
            Self::Timeline(_) => &[],
            Self::TraceRemoval(_) => &[],
        }
    }
}

pub(crate) struct Outcome {
    pub(crate) progress: Vec<String>,
    pub(crate) warnings: Vec<String>,
    pub(crate) report: QueryReport,
}

/// The cache generation and scope shared by material selection and excerpts.
struct Context {
    session: QuerySession,
    scope: Option<scope::ResolvedSearchScope>,
}

impl Context {
    fn open(options: SearchScopeOptions) -> Result<Self, AppError> {
        let repository = Repository::discover()?;
        let session = cache::open_query(&repository.root)?;
        let scope = scope::resolve(&session, options)?;
        Ok(Self { session, scope })
    }

    fn for_target(
        session: QuerySession,
        options: SearchScopeOptions,
        revision: &str,
    ) -> Result<Self, AppError> {
        let scope = scope::resolve_for_target(&session, options, revision)?;
        Ok(Self { session, scope })
    }

    fn filter(&self) -> Option<&SearchFilter> {
        self.scope.as_ref().map(|scope| &scope.filter)
    }

    /// Eligibility is distinct from traversal: why and timeline must follow
    /// the complete target history before filtering material or pagination.
    fn eligible_revisions(&self, revision: &str) -> Result<Option<HashSet<String>>, AppError> {
        self.filter()
            .map(|filter| self.session.scoped_revisions(filter, revision))
            .transpose()
    }

    fn intersect(&self, revision: &str, reachable: &mut HashSet<String>) -> Result<(), AppError> {
        if let Some(eligible) = self.eligible_revisions(revision)? {
            reachable.retain(|revision| eligible.contains(revision));
        }
        Ok(())
    }

    fn finish(self, mut report: QueryReport) -> Outcome {
        let scope = self.scope.map(|scope| scope.report);
        match &mut report {
            QueryReport::Followups(_) => {}
            QueryReport::Context(report) => report.scope = scope,
            QueryReport::Analysis(report) => report.scope = scope,
            QueryReport::Timeline(report) => report.scope = scope,
            QueryReport::TraceRemoval(report) => report.scope = scope,
        }
        Outcome {
            progress: self.session.progress().to_vec(),
            warnings: self.session.warnings().to_vec(),
            report,
        }
    }
}

pub(crate) fn execute(request: Request, options: Options) -> Result<Outcome, AppError> {
    if options.limit == 0 {
        return Err(AppError::input("limit must be greater than zero"));
    }
    match request {
        Request::Followups {
            revision,
            paths,
            to_rev,
            days,
            max_commits,
        } => followups::run(revision, paths, to_rev, days, max_commits, options),
        Request::Context { staged, hybrid } => run_context(staged, hybrid, options),
        Request::Search { words, hybrid } => {
            if hybrid {
                run_hybrid_search(words, options)
            } else {
                run_search(words, options)
            }
        }
        Request::CodeSearch {
            query,
            path,
            direction,
        } => run_code_search(query, path, direction, options),
        Request::TraceRemoval { query, path } => {
            let context = Context::open(options.scope)?;
            let report = capabilities::trace_removal::run(
                &context.session,
                &query,
                path.as_deref(),
                options.limit,
                context.filter(),
            )?;
            Ok(context.finish(QueryReport::TraceRemoval(report)))
        }
        Request::Examples { words, paths } => {
            run_text(words, paths, options, capabilities::examples)
        }
        Request::Failures { words, paths } => {
            run_text(words, paths, options, capabilities::failures)
        }
        Request::Related(paths) => run_paths(paths, options, capabilities::related),
        Request::Tests(paths) => run_paths(paths, options, capabilities::tests),
        Request::Regression {
            words,
            path,
            symbol,
            good,
            bad,
        } => run_regression(words, path, symbol, good, bad, options),
        Request::Why {
            revision,
            path,
            anchor,
        } => run_why(revision, path, anchor, options),
        Request::TraceFix { revision, paths } => run_trace_fix(revision, paths, options),
        Request::Timeline {
            path,
            at,
            offset,
            last,
        } => run_timeline(path, at, offset, last, options),
    }
}

fn run_context(staged: bool, hybrid: bool, options: Options) -> Result<Outcome, AppError> {
    let repository = Repository::discover()?;
    let input = repository.current_change(staged)?;
    if input.changes.is_empty() {
        scope::validate_time_bounds(&options.scope)?;
        let mut report = super::ContextReport::empty(input);
        report.semantic_requested = hybrid;
        return Ok(Outcome {
            progress: Vec::new(),
            warnings: Vec::new(),
            report: QueryReport::Context(report),
        });
    }
    let context = Context::open(options.scope)?;
    if hybrid {
        context.session.require_semantic_ready()?;
    }
    let report = capabilities::context::run(
        &context.session,
        input,
        &repository.root,
        options.limit,
        context.filter(),
        hybrid,
    )?;
    Ok(context.finish(QueryReport::Context(report)))
}
fn run_text(
    words: Vec<String>,
    paths: Vec<String>,
    options: Options,
    capability: impl FnOnce(
        &QuerySession,
        &Intent,
        usize,
        Option<&SearchFilter>,
    ) -> Result<Report, AppError>,
) -> Result<Outcome, AppError> {
    let intent = Intent::parse(&words, &paths)?;
    let context = Context::open(options.scope)?;
    let mut report = capability(&context.session, &intent, options.limit, context.filter())?;
    if options.patch {
        patch::attach_patch_excerpts(
            &context.session,
            &intent,
            &mut report,
            !paths.is_empty(),
            &paths,
        )?;
    }
    Ok(context.finish(QueryReport::Analysis(report)))
}

fn run_search(words: Vec<String>, options: Options) -> Result<Outcome, AppError> {
    let intent = Intent::parse(&words, &[])?;
    let context = Context::open(options.scope)?;
    let mut report = match context.filter() {
        Some(filter) => {
            capabilities::search_scoped(&context.session, &intent, options.limit, filter)?
        }
        None => capabilities::search(&context.session, &intent, options.limit)?,
    };
    if options.patch {
        patch::attach_patch_excerpts(&context.session, &intent, &mut report, false, &[])?;
    }
    Ok(context.finish(QueryReport::Analysis(report)))
}

fn run_hybrid_search(words: Vec<String>, options: Options) -> Result<Outcome, AppError> {
    let intent = Intent::parse(&words, &[])?;
    let context = Context::open(options.scope)?;
    context.session.require_semantic_ready()?;

    let query = words.join(" ");
    let (mut encoder, inputs) = Encoder::load_for_query(&query)?;
    let query_vectors = encoder.embed_query_chunks(&inputs)?;
    let mut report = capabilities::hybrid_search(
        &context.session,
        &intent,
        options.limit,
        context.filter(),
        &query_vectors,
    )?;
    if options.patch {
        patch::attach_patch_excerpts(&context.session, &intent, &mut report, false, &[])?;
    }
    Ok(context.finish(QueryReport::Analysis(report)))
}

fn run_code_search(
    query: String,
    path: Option<String>,
    direction: Option<CodeDirection>,
    options: Options,
) -> Result<Outcome, AppError> {
    let context = Context::open(options.scope)?;
    let report = match context.filter() {
        Some(filter) => capabilities::code_search_scoped(
            &context.session,
            &query,
            path.as_deref(),
            direction,
            options.limit,
            filter,
        )?,
        None => capabilities::code_search(
            &context.session,
            &query,
            path.as_deref(),
            direction,
            options.limit,
        )?,
    };
    Ok(context.finish(QueryReport::Analysis(report)))
}

fn run_paths(
    paths: Vec<String>,
    options: Options,
    capability: impl FnOnce(
        &QuerySession,
        &Intent,
        &std::path::Path,
        usize,
        Option<&SearchFilter>,
    ) -> Result<Report, AppError>,
) -> Result<Outcome, AppError> {
    let intent = Intent::paths(&paths)?;
    let context = Context::open(options.scope)?;
    let report = capability(
        &context.session,
        &intent,
        context.session.root(),
        options.limit,
        context.filter(),
    )?;
    Ok(context.finish(QueryReport::Analysis(report)))
}

fn run_regression(
    words: Vec<String>,
    path: String,
    symbol: Option<String>,
    good: Option<String>,
    bad: Option<String>,
    options: Options,
) -> Result<Outcome, AppError> {
    let intent = Intent::symptom(&words, &path)?;
    let repository = Repository::discover()?;
    let session = cache::open_query(&repository.root)?;
    let bad = match bad {
        Some(revision) => revision,
        None => session.completed_tip()?,
    };
    let target =
        repository.pin_regression_target(&bad, good.as_deref(), &path, symbol.as_deref())?;
    session.require_revision(&target.bad_revision)?;
    if let Some(good_revision) = &target.good_revision {
        session.require_revision(good_revision)?;
    }
    let context = Context::for_target(session, options.scope, &target.bad_revision)?;
    let bad_reachable = context.session.ancestors(&target.bad_revision)?;
    let mut reachable = if let Some(good_revision) = &target.good_revision {
        let good_reachable = context.session.ancestors(good_revision)?;
        bad_reachable.difference(&good_reachable).cloned().collect()
    } else {
        bad_reachable
    };
    context.intersect(&target.bad_revision, &mut reachable)?;
    let report = capabilities::regression(
        &context.session,
        &intent,
        &target,
        &reachable,
        options.limit,
        options.patch,
    )?;
    Ok(context.finish(QueryReport::Analysis(report)))
}

fn run_why(
    revision: Option<String>,
    path: String,
    anchor: WhyAnchor,
    options: Options,
) -> Result<Outcome, AppError> {
    let repository = Repository::discover()?;
    let session = cache::open_query(&repository.root)?;
    let revision = match revision {
        Some(revision) => revision,
        None => session.completed_tip()?,
    };
    let target = repository.pin_why_target(&revision, &path, anchor)?;
    session.require_revision(&target.revision)?;
    let context = Context::for_target(session, options.scope, &target.revision)?;
    let reachable = context.session.ancestors(&target.revision)?;
    let eligible = context.eligible_revisions(&target.revision)?;
    let report = capabilities::why(
        &context.session,
        &target,
        &reachable,
        eligible.as_ref(),
        context.scope.as_ref().map(|scope| &scope.report),
        options.limit,
        options.patch,
    )?;
    Ok(context.finish(QueryReport::Analysis(report)))
}

fn run_timeline(
    path: String,
    at: Option<String>,
    offset: usize,
    last: bool,
    options: Options,
) -> Result<Outcome, AppError> {
    let repository = Repository::discover()?;
    let session = cache::open_query(&repository.root)?;
    let revision = match at {
        Some(revision) => revision,
        None => session.completed_tip()?,
    };
    let target = repository.pin_timeline_target(&revision, &path)?;
    session.require_revision(&target.revision)?;
    let context = Context::for_target(session, options.scope, &target.revision)?;
    let reachable = context.session.ancestors(&target.revision)?;
    let eligible = context.eligible_revisions(&target.revision)?;
    let history = context.session.timeline_history(&target.path, &reachable)?;
    let mut report = TimelineReport::from_history(
        target.revision,
        target.path,
        history,
        eligible.as_ref(),
        options.limit,
        offset,
        last,
    );
    if options.patch {
        patch::attach_timeline_patch_excerpts(&context.session, &mut report)?;
    }
    Ok(context.finish(QueryReport::Timeline(report)))
}

fn run_trace_fix(
    revision: String,
    paths: Vec<String>,
    options: Options,
) -> Result<Outcome, AppError> {
    let repository = Repository::discover()?;
    let target = repository.pin_trace_fix(&revision, &paths)?;
    let session = cache::open_query(&repository.root)?;
    session.require_revision(&target.revision)?;
    let context = Context::for_target(session, options.scope, &target.revision)?;
    let mut reachable = context.session.ancestors(&target.revision)?;
    context.intersect(&target.revision, &mut reachable)?;
    let mut report = capabilities::trace_fix(
        &context.session,
        &target,
        &reachable,
        options.limit,
        context.scope.is_some(),
    )?;
    if options.patch {
        patch::attach_trace_fix_patch_excerpts(&context.session, &mut report)?;
    }
    Ok(context.finish(QueryReport::Analysis(report)))
}
