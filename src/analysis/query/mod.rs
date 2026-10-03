//! Execute a historical query against one published cache generation.
//!
//! Target pinning, scope semantics, material assembly and optional excerpts stay
//! behind this interface. Display formats never participate in query execution.

mod context;
pub(in crate::analysis) mod scope;

use super::{CodeDirection, Intent, Report, capabilities, patch};
use crate::{
    app::AppError,
    cache::{QuerySession, SearchFilter},
    git::WhyAnchor,
};

pub(in crate::analysis) use context::Context;

pub(crate) use scope::SearchScopeOptions;

pub(crate) enum Request {
    Conflicts {
        paths: Vec<String>,
    },
    Followups {
        revision: String,
        paths: Vec<String>,
        to_rev: Option<String>,
        days: usize,
        max_commits: usize,
    },
    Hotspots {
        path_prefix: Option<String>,
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
    CodeRegexSearch {
        pattern: String,
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
    Patterns {
        paths: Vec<String>,
        min_support: usize,
    },
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
    Conflicts(capabilities::conflicts::Report),
    Followups(capabilities::followups::Report),
    Context(super::ContextReport),
    Patterns(capabilities::patterns::Report),
    Analysis(Report),
    Timeline(super::TimelineReport),
    TraceRemoval(super::TraceRemovalReport),
    Hotspots(super::HotspotsReport),
}

impl QueryReport {
    pub(crate) fn warnings(&self) -> &[String] {
        match self {
            Self::Conflicts(report) => &report.warnings,
            Self::Followups(report) => &report.warnings,
            Self::Patterns(_) => &[],
            Self::Context(report) => &report.warnings,
            Self::Analysis(report) => &report.warnings,
            Self::Timeline(_) | Self::Hotspots(_) | Self::TraceRemoval(_) => &[],
        }
    }

    pub(crate) fn notices(&self) -> &[String] {
        match self {
            Self::Conflicts(_) => &[],
            Self::Followups(_) => &[],
            Self::Patterns(_) => &[],
            Self::Context(_) => &[],
            Self::Analysis(report) => &report.notices,
            Self::Timeline(_) | Self::Hotspots(_) | Self::TraceRemoval(_) => &[],
        }
    }
}

pub(crate) struct Outcome {
    pub(crate) progress: Vec<String>,
    pub(crate) warnings: Vec<String>,
    pub(crate) report: QueryReport,
}

pub(crate) fn execute(request: Request, options: Options) -> Result<Outcome, AppError> {
    if options.limit == 0 {
        return Err(AppError::input("limit must be greater than zero"));
    }
    match request {
        Request::Conflicts { paths } => capabilities::conflicts::execute(paths, options.limit),
        Request::Followups {
            revision,
            paths,
            to_rev,
            days,
            max_commits,
        } => capabilities::followups::run(revision, paths, to_rev, days, max_commits, options),
        Request::Context { staged, hybrid } => {
            capabilities::context::execute(staged, hybrid, options)
        }
        Request::Hotspots { path_prefix } => capabilities::hotspots::execute(options, path_prefix),
        Request::Search { words, hybrid } => {
            if hybrid {
                capabilities::hybrid::execute(words, options)
            } else {
                capabilities::search::execute(words, options)
            }
        }
        Request::CodeSearch {
            query,
            path,
            direction,
        } => capabilities::code_search::execute(query, path, direction, options),
        Request::CodeRegexSearch {
            pattern,
            path,
            direction,
        } => capabilities::code_search::execute_regex(pattern, path, direction, options),
        Request::TraceRemoval { query, path } => {
            capabilities::trace_removal::execute(query, path, options)
        }
        Request::Examples { words, paths } => {
            run_text(words, paths, options, capabilities::examples)
        }
        Request::Failures { words, paths } => {
            run_text(words, paths, options, capabilities::failures)
        }
        Request::Related(paths) => run_paths(paths, options, capabilities::related),
        Request::Patterns { paths, min_support } => {
            capabilities::patterns::execute(paths, min_support, options)
        }
        Request::Tests(paths) => run_paths(paths, options, capabilities::tests),
        Request::Regression {
            words,
            path,
            symbol,
            good,
            bad,
        } => capabilities::regression::execute(words, path, symbol, good, bad, options),
        Request::Why {
            revision,
            path,
            anchor,
        } => capabilities::why::execute(revision, path, anchor, options),
        Request::TraceFix { revision, paths } => {
            capabilities::trace_fix::execute(revision, paths, options)
        }
        Request::Timeline {
            path,
            at,
            offset,
            last,
        } => capabilities::timeline::execute(path, at, offset, last, options),
    }
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
