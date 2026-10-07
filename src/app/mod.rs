mod clear;
mod error;
mod index;
mod prune;
mod update;

use crate::github;
use crate::{
    analysis::{
        CodeDirection,
        capabilities::usage::{self, GroupBy},
        query::{self, Options, Request, SearchScopeOptions},
    },
    cli::{CodeChange, Command, HistoricalScopeArgs, StatsGroup},
    git::WhyAnchor,
};

pub(crate) use error::AppError;

#[derive(Clone, Copy)]
pub(crate) enum IndexStage {
    ReadingCommits,
    ReadingChanges,
    ReadingPatches,
    WritingCache,
    Complete,
    BuildingSemanticIndex,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UpdateStage {
    Checking,
    Downloading,
    Verifying,
    Installing,
}

pub(crate) enum Progress {
    Index(IndexStage),
    Update(UpdateStage),
}

pub(crate) struct IndexReport {
    /// Requested revision paired with its resolved, deduplicated commit ID;
    /// aliases of one commit repeat the commit ID. Empty when indexing HEAD.
    pub(crate) selected: Vec<(String, String)>,
    /// Unique commits reachable from the selected histories.
    pub(crate) selected_commit_count: usize,
    pub(crate) semantic_disabled: bool,
}

pub(crate) struct Outcome {
    pub(crate) progress: Vec<String>,
    pub(crate) warnings: Vec<String>,
    pub(crate) message: String,
    pub(crate) notices: Vec<String>,
    pub(crate) report: Option<query::QueryReport>,
    pub(crate) usage_report: Option<usage::Report>,
    pub(crate) github_links: Option<crate::github::LinksReport>,
    pub(crate) clear_report: Option<crate::cache::ClearReport>,
    pub(crate) index_report: Option<IndexReport>,
    pub(crate) prune_report: Option<crate::cache::PruneReport>,
}

impl From<HistoricalScopeArgs> for SearchScopeOptions {
    fn from(options: HistoricalScopeArgs) -> Self {
        Self {
            from_rev: options.from_rev,
            to_rev: options.to_rev,
            since: options.since,
            until: options.until,
            paths: Vec::new(),
        }
    }
}

/// Code modes keep a single exact historical path; QUERY mode carries repeatable
/// paths as scope options.
fn require_single_path(paths: &[String], mode: &str) -> Result<(), AppError> {
    if paths.len() > 1 {
        return Err(AppError::input(format!(
            "error: --path with {mode} accepts at most one path; {} were given",
            paths.len()
        )));
    }
    Ok(())
}

pub(crate) fn execute(
    command: Command,
    report: &mut dyn FnMut(Progress),
) -> Result<Outcome, AppError> {
    let command = match command {
        Command::Stats {
            all,
            since,
            until,
            group,
            ..
        } => {
            let group = match group {
                StatsGroup::Day => GroupBy::Day,
                StatsGroup::Week => GroupBy::Week,
                StatsGroup::Month => GroupBy::Month,
            };
            let usage_report = usage::report(all, since.as_deref(), until.as_deref(), group)
                .map_err(|error| match error {
                    usage::Error::Input(message) => AppError::input(format!("error: {message}")),
                    usage::Error::Storage(message) => AppError::operational(format!(
                        "error: reading local usage statistics: {message}"
                    )),
                })?;
            return Ok(Outcome {
                progress: Vec::new(),
                warnings: Vec::new(),
                message: String::new(),
                notices: Vec::new(),
                report: None,
                usage_report: Some(usage_report),
                github_links: None,
                clear_report: None,
                index_report: None,
                prune_report: None,
            });
        }
        command => command,
    };
    let github_link_request = match &command {
        Command::Search {
            github_links,
            github_repo,
            ..
        } => (*github_links).then(|| github_repo.clone()),
        Command::Timeline {
            github_links,
            github_repo,
            ..
        } => (*github_links).then(|| github_repo.clone()),
        Command::Examples { github, .. }
        | Command::Failures { github, .. }
        | Command::Related { github, .. }
        | Command::Tests { github, .. }
        | Command::Regression { github, .. }
        | Command::Why { github, .. }
        | Command::TraceFix { github, .. } => {
            github.github_links.then(|| github.github_repo.clone())
        }
        _ => None,
    };
    let (request, limit, patch, scope) = match command {
        Command::Followups {
            revision,
            paths,
            to_rev,
            days,
            max_commits,
            limit,
            patch,
            verbose,
            ..
        } => (
            Request::Followups {
                revision,
                paths,
                to_rev,
                days,
                max_commits,
                verbose,
            },
            limit,
            patch,
            SearchScopeOptions::default(),
        ),
        Command::Index {
            refs,
            semantic,
            no_semantic,
        } => {
            return index::run(refs, semantic, no_semantic, &mut |stage| {
                report(Progress::Index(stage))
            });
        }
        Command::Clear { dry_run } => return clear::run(dry_run),
        Command::Prune { dry_run } => return prune::run(dry_run),
        Command::Update => return update::run(&mut |stage| report(Progress::Update(stage))),
        Command::Hotspots {
            limit,
            path_prefix,
            scope,
            ..
        } => (
            Request::Hotspots { path_prefix },
            limit,
            false,
            scope.into(),
        ),
        Command::Search {
            query,
            patch_of,
            max_patch_checks,
            code,
            code_regex,
            code_file,
            hybrid,
            change,
            paths,
            limit,
            patch,
            scope,
            ..
        } => {
            let direction = change.map(|change| match change {
                CodeChange::Added => CodeDirection::Added,
                CodeChange::Removed => CodeDirection::Removed,
            });
            let is_query_mode = query.is_some();
            let code_path = if is_query_mode {
                None
            } else {
                require_single_path(&paths, "code modes")?;
                paths.first().cloned()
            };
            let request = match (query, code, code_regex, code_file, patch_of) {
                (None, None, None, None, Some(revision)) => Request::PatchSearch {
                    revision,
                    max_patch_checks,
                },
                (Some(words), None, None, None, None) => Request::Search { words, hybrid },
                (None, Some(query), None, None, None) => Request::CodeSearch {
                    query,
                    path: code_path,
                    direction,
                },
                (None, None, Some(pattern), None, None) => Request::CodeRegexSearch {
                    pattern,
                    path: code_path,
                    direction,
                },
                (None, None, None, Some(input), None) => Request::FragmentSearch {
                    input,
                    path: code_path,
                    direction,
                },
                _ => unreachable!("clap enforces exactly one search mode"),
            };
            let mut options = SearchScopeOptions::from(scope);
            if is_query_mode {
                options.paths = paths;
            }
            (request, limit, patch, options)
        }
        Command::TraceRemoval {
            code,
            code_file,
            path,
            limit,
            scope,
            ..
        } => {
            let request = match (code, code_file) {
                (Some(query), None) => Request::TraceRemoval { query, path },
                (None, Some(input)) => Request::TraceRemovalFragment { input, path },
                _ => unreachable!("clap enforces exactly one trace-removal mode"),
            };
            (request, limit, false, scope.into())
        }
        Command::Examples {
            query: words,
            paths,
            limit,
            patch,
            scope,
            ..
        } => (
            Request::Examples { words, paths },
            limit,
            patch,
            scope.into(),
        ),
        Command::Failures {
            query: words,
            paths,
            limit,
            scope,
            ..
        } => (
            Request::Failures { words, paths },
            limit,
            false,
            scope.into(),
        ),
        Command::Conflicts {
            paths,
            limit,
            max_historical_checks,
            ..
        } => (
            Request::Conflicts {
                paths,
                max_historical_checks,
            },
            limit,
            false,
            Default::default(),
        ),
        Command::Propagation {
            source, targets, ..
        } => (
            Request::Propagation { source, targets },
            1,
            false,
            Default::default(),
        ),
        Command::Context {
            staged,
            hybrid,
            no_historical_followup,
            followup_days,
            max_followup_checks,
            limit,
            scope,
            ..
        } => (
            Request::Context {
                staged,
                hybrid,
                historical_followup: !no_historical_followup,
                followup_days,
                max_followup_checks,
            },
            limit,
            false,
            scope.into(),
        ),
        Command::Related {
            paths,
            patterns,
            min_support,
            line,
            symbol,
            at,
            limit,
            scope,
            ..
        } => (
            if patterns {
                Request::Patterns {
                    paths,
                    min_support: min_support.unwrap_or(3),
                }
            } else if let Some(line) = line {
                Request::RelatedTarget {
                    paths,
                    anchor: WhyAnchor::Line { number: line },
                    at,
                }
            } else if let Some(symbol) = symbol {
                Request::RelatedTarget {
                    paths,
                    anchor: WhyAnchor::Symbol {
                        name: symbol,
                        number: 0,
                    },
                    at,
                }
            } else {
                Request::Related(paths)
            },
            limit,
            false,
            scope.into(),
        ),
        Command::Tests {
            paths,
            target,
            limit,
            scope,
            ..
        } => {
            let request = if let Some(line) = target.line {
                Request::TestsTarget {
                    paths,
                    anchor: WhyAnchor::Line { number: line },
                    at: target.at,
                }
            } else if let Some(symbol) = target.symbol {
                Request::TestsTarget {
                    paths,
                    anchor: WhyAnchor::Symbol {
                        name: symbol,
                        number: 0,
                    },
                    at: target.at,
                }
            } else {
                Request::Tests(paths)
            };
            (request, limit, false, scope.into())
        }
        Command::Regression {
            symptom: words,
            path,
            symbol,
            good,
            bad,
            limit,
            patch,
            scope,
            ..
        } => (
            Request::Regression {
                words,
                path,
                symbol,
                good,
                bad,
            },
            limit,
            patch,
            scope.into(),
        ),
        Command::Why {
            path,
            line,
            symbol,
            at,
            from_rev,
            to_rev,
            since,
            until,
            limit,
            patch,
            ..
        } => {
            let anchor = match (line, symbol) {
                (Some(number), None) => WhyAnchor::Line { number },
                (None, Some(name)) => WhyAnchor::Symbol { name, number: 0 },
                _ => unreachable!("clap enforces exactly one why anchor"),
            };
            (
                Request::Why {
                    revision: at,
                    path,
                    anchor,
                },
                limit,
                patch,
                SearchScopeOptions {
                    from_rev,
                    to_rev,
                    since,
                    until,
                    paths: Vec::new(),
                },
            )
        }
        Command::TraceFix {
            fix_revision,
            paths,
            limit,
            patch,
            scope,
            ..
        } => (
            Request::TraceFix {
                revision: fix_revision,
                paths,
            },
            limit,
            patch,
            scope.into(),
        ),
        Command::Timeline {
            path,
            at,
            from_rev,
            to_rev,
            since,
            until,
            limit,
            offset,
            last,
            patch,
            ..
        } => (
            Request::Timeline {
                path,
                at,
                offset: offset.unwrap_or(0),
                last,
            },
            limit,
            patch,
            SearchScopeOptions {
                from_rev,
                to_rev,
                since,
                until,
                paths: Vec::new(),
            },
        ),
        Command::Fate {
            path,
            line,
            symbol,
            at,
            to_rev,
            max_commits,
            limit,
            patch,
            ..
        } => (
            Request::Fate {
                path,
                line,
                symbol,
                at,
                to_rev,
                max_commits,
            },
            limit,
            patch,
            SearchScopeOptions::default(),
        ),
        Command::Stats { .. } => unreachable!("stats command handled before query dispatch"),
    };
    let result = query::execute(
        request,
        Options {
            limit,
            patch,
            scope,
        },
    )?;
    let github_links = github_link_request.map(|explicit_repo| match &result.report {
        query::QueryReport::PatchSearch(_) => {
            unreachable!("patch search has no GitHub link option")
        }
        query::QueryReport::Conflicts(_) => unreachable!("conflicts has no GitHub link option"),
        query::QueryReport::Context(_) | query::QueryReport::Followups(_) => {
            unreachable!("context has no GitHub link option")
        }
        query::QueryReport::TraceRemoval(_) | query::QueryReport::TraceRemovalFragment(_) => {
            unreachable!("trace-removal has no GitHub link option")
        }
        query::QueryReport::Patterns(_) => unreachable!("patterns has no GitHub link option"),
        query::QueryReport::Hotspots(_) | query::QueryReport::Propagation(_) => {
            unreachable!("propagation has no GitHub link option")
        }
        query::QueryReport::Analysis(report) => github::fetch(report, explicit_repo.as_deref()),
        query::QueryReport::FragmentSearch(report) => {
            github::fetch_fragments(report, explicit_repo.as_deref())
        }
        query::QueryReport::Timeline(report) => {
            github::fetch_timeline(report, explicit_repo.as_deref())
        }
        query::QueryReport::Fate(_) => unreachable!("fate has no GitHub link option"),
    });
    Ok(Outcome {
        progress: result.progress,
        warnings: result.warnings,
        message: String::new(),
        notices: result.report.notices().to_vec(),
        report: Some(result.report),
        usage_report: None,
        github_links,
        clear_report: None,
        index_report: None,
        prune_report: None,
    })
}
