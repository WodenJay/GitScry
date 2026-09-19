use crate::{analysis, cache, git::Repository, render};

use super::{AppError, Outcome, prepare_cache};

/// One query command: build the intent, prepare the cache, run the capability, render it.
pub(super) fn run(
    words: Vec<String>,
    paths: Vec<String>,
    limit: usize,
    capability: impl FnOnce(
        &rusqlite::Connection,
        &analysis::Intent,
        usize,
    ) -> Result<analysis::Report, AppError>,
) -> Result<Outcome, AppError> {
    let intent = analysis::Intent::parse(&words, &paths)?;
    let repository = Repository::discover()?;
    let prepared = prepare_cache(&repository)?;
    let connection = cache::open(&repository.root)?;
    let mut report = capability(&connection, &intent, limit)?;
    let mut progress = prepared.progress;
    progress.append(&mut report.warnings);
    Ok(Outcome {
        progress,
        message: render::format_report(&report),
    })
}

/// One path query: build the intent, prepare the cache, run the capability, render it.
pub(super) fn run_paths(
    paths: Vec<String>,
    limit: usize,
    capability: impl FnOnce(
        &rusqlite::Connection,
        &analysis::Intent,
        &[Vec<u8>],
        usize,
    ) -> Result<analysis::Report, AppError>,
) -> Result<Outcome, AppError> {
    let intent = analysis::Intent::paths(&paths)?;
    let repository = Repository::discover()?;
    let prepared = prepare_cache(&repository)?;
    let connection = cache::open(&repository.root)?;
    let worktree_paths = repository.worktree_paths()?;
    let mut report = capability(&connection, &intent, &worktree_paths, limit)?;
    let mut progress = prepared.progress;
    progress.append(&mut report.warnings);
    Ok(Outcome {
        progress,
        message: render::format_report(&report),
    })
}
