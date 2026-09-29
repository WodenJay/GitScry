mod error;
mod index;
mod query;

mod update;
use crate::{
    analysis,
    cli::{CodeChange, Command},
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
pub(crate) enum QueryReport {
    Analysis(analysis::Report),
    Timeline(crate::timeline::Report),
}

impl QueryReport {
    pub(crate) fn warnings(&self) -> &[String] {
        match self {
            Self::Analysis(report) => &report.warnings,
            Self::Timeline(_) => &[],
        }
    }

    pub(crate) fn notices(&self) -> &[String] {
        match self {
            Self::Analysis(report) => &report.notices,
            Self::Timeline(_) => &[],
        }
    }
}

pub(crate) struct Outcome {
    pub(crate) progress: Vec<String>,
    pub(crate) warnings: Vec<String>,
    pub(crate) message: String,
    pub(crate) notices: Vec<String>,
    pub(crate) report: Option<QueryReport>,
}

pub(crate) fn execute(
    command: Command,
    json_output: bool,
    report: &mut dyn FnMut(Progress),
) -> Result<Outcome, AppError> {
    match command {
        Command::Index => index::run(&mut |stage| report(Progress::Index(stage))),
        Command::Update => update::run(&mut |stage| report(Progress::Update(stage))),
        Command::Search {
            query,
            code,
            change,
            path,
            limit,
            patch,
            ..
        } => match (query, code) {
            (Some(query), None) if patch => {
                query::run_with_patch(query, Vec::new(), limit, false, analysis::search)
            }
            (Some(query), None) => query::run(query, Vec::new(), limit, analysis::search),
            (None, Some(code)) => {
                let direction = change.map(|change| match change {
                    CodeChange::Added => analysis::CodeDirection::Added,
                    CodeChange::Removed => analysis::CodeDirection::Removed,
                });
                query::run_code(code, path, direction, limit)
            }
            _ => unreachable!("clap enforces exactly one search mode"),
        },
        Command::Examples {
            query,
            paths,
            limit,
            patch,
            ..
        } => {
            if patch {
                let path_only = !paths.is_empty();
                query::run_with_patch(query, paths, limit, path_only, analysis::examples)
            } else {
                query::run(query, paths, limit, analysis::examples)
            }
        }
        Command::Failures {
            query,
            paths,
            limit,
            ..
        } => query::run(query, paths, limit, analysis::failures),
        Command::Related { paths, limit, .. } => {
            query::run_paths(paths, limit, json_output, analysis::related)
        }
        Command::Tests { paths, limit, .. } => {
            query::run_paths(paths, limit, json_output, analysis::tests)
        }
        Command::Regression {
            symptom,
            path,
            symbol,
            good,
            bad,
            limit,
            ..
        } => query::run_regression(symptom, path, symbol, good, bad, limit),
        Command::Why {
            path,
            line,
            symbol,
            at,
            limit,
            ..
        } => {
            let anchor = match (line, symbol) {
                (Some(number), None) => crate::git::WhyAnchor::Line { number },
                (None, Some(name)) => crate::git::WhyAnchor::Symbol { name, number: 0 },
                _ => unreachable!("clap enforces exactly one why anchor"),
            };
            query::run_why(at, path, anchor, limit)
        }
        Command::TraceFix {
            fix_revision,
            paths,
            limit,
            ..
        } => query::run_trace_fix(fix_revision, paths, limit),
        Command::Timeline {
            path,
            at,
            limit,
            offset,
            last,
            ..
        } => query::run_timeline(path, at, limit, offset.unwrap_or(0), last),
    }
}
