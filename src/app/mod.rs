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

pub(crate) struct Outcome {
    pub(crate) progress: Vec<String>,
    pub(crate) warnings: Vec<String>,
    pub(crate) message: String,
    pub(crate) notices: Vec<String>,
    pub(crate) report: Option<analysis::Report>,
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
            ..
        } => match (query, code) {
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
            ..
        } => query::run(query, paths, limit, analysis::examples),
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
    }
}
