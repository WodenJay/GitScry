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
    FragmentSearch {
        input: std::path::PathBuf,
        path: Option<String>,
        direction: Option<CodeDirection>,
    },
    Conflicts {
        paths: Vec<String>,
        max_historical_checks: Option<usize>,
    },
    Followups {
        revision: String,
        paths: Vec<String>,
        to_rev: Option<String>,
        days: usize,
        max_commits: usize,
        verbose: bool,
    },
    Hotspots {
        path_prefix: Option<String>,
    },
    Context {
        staged: bool,
        hybrid: bool,
        historical_followup: bool,
        followup_days: usize,
        max_followup_checks: Option<usize>,
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
    TraceRemovalFragment {
        input: std::path::PathBuf,
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
    RelatedTarget {
        paths: Vec<String>,
        anchor: WhyAnchor,
        at: Option<String>,
    },
    TestsTarget {
        paths: Vec<String>,
        anchor: WhyAnchor,
        at: Option<String>,
    },
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
    Propagation {
        source: String,
        targets: Vec<String>,
    },
    Fate {
        path: String,
        line: usize,
        at: String,
        to_rev: Option<String>,
        max_commits: Option<usize>,
    },
}

pub(crate) struct Options {
    pub(crate) limit: usize,
    pub(crate) patch: bool,
    pub(crate) scope: SearchScopeOptions,
}

pub(crate) enum QueryReport {
    FragmentSearch(capabilities::fragment_search::Report),
    Conflicts(capabilities::conflicts::Report),
    Followups(capabilities::followups::Report),
    Context(super::ContextReport),
    Patterns(capabilities::patterns::Report),
    Analysis(Report),
    Timeline(super::TimelineReport),
    TraceRemoval(super::TraceRemovalReport),
    TraceRemovalFragment(super::TraceRemovalFragmentReport),
    Hotspots(super::HotspotsReport),
    Propagation(capabilities::propagation::Report),
    Fate(capabilities::fate::Report),
}

impl QueryReport {
    /// Warnings for human-readable stderr. Mirrors `warnings` for every report
    /// except those that present dynamic diagnostics differently by default.
    pub(crate) fn human_warnings(&self) -> std::borrow::Cow<'_, [String]> {
        match self {
            Self::Followups(report) => report.human_warnings(),
            _ => std::borrow::Cow::Borrowed(self.warnings()),
        }
    }

    pub(crate) fn warnings(&self) -> &[String] {
        match self {
            Self::FragmentSearch(_) => &[],
            Self::Conflicts(report) => &report.warnings,
            Self::Followups(report) => &report.warnings,
            Self::Patterns(_) => &[],
            Self::Context(report) => &report.warnings,
            Self::Analysis(report) => &report.warnings,
            Self::Timeline(_)
            | Self::Hotspots(_)
            | Self::TraceRemoval(_)
            | Self::TraceRemovalFragment(_)
            | Self::Propagation(_)
            | Self::Fate(_) => &[],
        }
    }

    pub(crate) fn notices(&self) -> &[String] {
        match self {
            Self::FragmentSearch(_) => &[],
            Self::Conflicts(_) => &[],
            Self::Followups(_) => &[],
            Self::Patterns(_) => &[],
            Self::Context(_) => &[],
            Self::Analysis(report) => &report.notices,
            Self::Timeline(_)
            | Self::Hotspots(_)
            | Self::TraceRemoval(_)
            | Self::TraceRemovalFragment(_)
            | Self::Propagation(_)
            | Self::Fate(_) => &[],
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
        Request::FragmentSearch {
            input,
            path,
            direction,
        } => capabilities::fragment_search::execute(input, path, direction, options),
        Request::Conflicts {
            paths,
            max_historical_checks,
        } => capabilities::conflicts::execute(paths, max_historical_checks, options.limit),
        Request::Propagation { source, targets } => {
            capabilities::propagation::execute(&source, &targets)
        }
        Request::Followups {
            revision,
            paths,
            to_rev,
            days,
            max_commits,
            verbose,
        } => capabilities::followups::run(
            revision,
            paths,
            to_rev,
            days,
            max_commits,
            verbose,
            options,
        ),
        Request::Context {
            staged,
            hybrid,
            historical_followup,
            followup_days,
            max_followup_checks,
        } => capabilities::context::execute(
            staged,
            hybrid,
            historical_followup,
            followup_days,
            max_followup_checks,
            options,
        ),
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
        Request::TraceRemovalFragment { input, path } => {
            capabilities::trace_removal::execute_fragment(input, path, options)
        }
        Request::Examples { words, paths } => {
            run_text(words, paths, options, capabilities::examples)
        }
        Request::Failures { words, paths } => {
            run_text(words, paths, options, capabilities::failures)
        }
        Request::Related(paths) => run_paths(paths, options, capabilities::related),
        Request::RelatedTarget { paths, anchor, at } => {
            capabilities::relations::execute_target(paths, anchor, at, options, false)
        }
        Request::TestsTarget { paths, anchor, at } => {
            capabilities::relations::execute_target(paths, anchor, at, options, true)
        }
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
        Request::Fate {
            path,
            line,
            at,
            to_rev,
            max_commits,
        } => capabilities::fate::execute(path, line, at, to_rev, max_commits, options),
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
