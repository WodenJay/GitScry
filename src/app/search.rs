use crate::{analysis, cache, git, render};

use super::{AppError, Outcome, prepare_cache};

pub(super) fn run(query: Vec<String>, limit: usize) -> Result<Outcome, AppError> {
    let query = query.join(" ");
    if query.trim().is_empty() {
        return Err(AppError::input("search query must not be empty"));
    }

    let repository = git::Repository::discover()?;
    let prepared = prepare_cache(&repository, false)?;
    let connection = cache::open(&repository.root)?;
    let report = analysis::search(&connection, &query, limit)?;

    Ok(Outcome {
        progress: prepared.progress,
        message: render::format_search(&report),
    })
}
