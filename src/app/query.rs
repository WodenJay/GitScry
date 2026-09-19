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
    let report = capability(&connection, &intent, limit)?;

    Ok(Outcome {
        progress: prepared.progress,
        message: render::format_report(&report),
    })
}
