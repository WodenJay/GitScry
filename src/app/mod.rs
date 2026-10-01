mod error;
mod index;
mod update;

use crate::github;
use crate::{
    analysis::{
        CodeDirection,
        query::{self, Options, Request, SearchScopeOptions},
    },
    cli::{CodeChange, Command, HistoricalScopeArgs},
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

pub(crate) struct Outcome {
    pub(crate) progress: Vec<String>,
    pub(crate) warnings: Vec<String>,
    pub(crate) message: String,
    pub(crate) notices: Vec<String>,
    pub(crate) report: Option<query::QueryReport>,
    pub(crate) github_links: Option<crate::github::LinksReport>,
}

impl From<HistoricalScopeArgs> for SearchScopeOptions {
    fn from(options: HistoricalScopeArgs) -> Self {
        Self {
            from_rev: options.from_rev,
            to_rev: options.to_rev,
            since: options.since,
            until: options.until,
        }
    }
}

pub(crate) fn execute(
    command: Command,
    report: &mut dyn FnMut(Progress),
) -> Result<Outcome, AppError> {
    let mut github_repository = None;
    let (request, limit, patch, scope) = match command {
        Command::Index => return index::run(&mut |stage| report(Progress::Index(stage))),
        Command::Update => return update::run(&mut |stage| report(Progress::Update(stage))),
        Command::Search {
            query,
            code,
            change,
            path,
            limit,
            patch,
            scope,
            github_links,
            github_repo,
            ..
        } => {
            let request = match (query, code) {
                (Some(words), None) => Request::Search(words),
                (None, Some(query)) => Request::CodeSearch {
                    query,
                    path,
                    direction: change.map(|change| match change {
                        CodeChange::Added => CodeDirection::Added,
                        CodeChange::Removed => CodeDirection::Removed,
                    }),
                },
                _ => unreachable!("clap enforces exactly one search mode"),
            };
            github_repository = github_links.then_some(github_repo);
            (request, limit, patch, scope.into())
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
        Command::Related {
            paths,
            limit,
            scope,
            ..
        } => (Request::Related(paths), limit, false, scope.into()),
        Command::Tests {
            paths,
            limit,
            scope,
            ..
        } => (Request::Tests(paths), limit, false, scope.into()),
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
            },
        ),
    };
    let result = query::execute(
        request,
        Options {
            limit,
            patch,
            scope,
        },
    )?;
    let github_links = github_repository.map(|explicit_repo| {
        let query::QueryReport::Analysis(report) = &result.report else {
            unreachable!("GitHub links are enabled only for text search");
        };
        github::fetch(report, explicit_repo.as_deref())
    });
    Ok(Outcome {
        progress: result.progress,
        warnings: result.warnings,
        message: String::new(),
        notices: result.report.notices().to_vec(),
        report: Some(result.report),
        github_links,
    })
}
