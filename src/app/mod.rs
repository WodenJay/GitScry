mod error;
mod index;
mod query;

use crate::{analysis, cli::Command};

pub(crate) use error::AppError;

#[derive(Clone, Copy)]
pub(crate) enum IndexStage {
    ReadingCommits,
    ReadingChanges,
    ReadingPatches,
    WritingCache,
    Complete,
}

pub(crate) struct Outcome {
    pub(crate) progress: Vec<String>,
    pub(crate) message: String,
    pub(crate) notices: Vec<String>,
}

pub(crate) fn execute(
    command: Command,
    report: &mut dyn FnMut(IndexStage),
) -> Result<Outcome, AppError> {
    match command {
        Command::Index => index::run(report),
        Command::Search { query, limit } => query::run(query, Vec::new(), limit, analysis::search),
        Command::Examples {
            query,
            paths,
            limit,
        } => query::run(query, paths, limit, analysis::examples),
        Command::Failures {
            query,
            paths,
            limit,
        } => query::run(query, paths, limit, analysis::failures),
        Command::Related { paths, limit } => query::run_paths(paths, limit, analysis::related),
        Command::Tests { paths, limit } => query::run_paths(paths, limit, analysis::tests),
        Command::Regression {
            symptom,
            path,
            symbol,
            good,
            bad,
            limit,
        } => query::run_regression(symptom, path, symbol, good, bad, limit),
        Command::Why {
            path,
            line,
            symbol,
            at,
            limit,
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
        } => query::run_trace_fix(fix_revision, paths, limit),
    }
}
