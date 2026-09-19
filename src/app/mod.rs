mod error;
mod index;
mod search;

use crate::{cache, cli::Command, git};

pub(crate) use error::AppError;

pub(crate) struct Outcome {
    pub(crate) progress: Vec<String>,
    pub(crate) message: String,
}

pub(crate) struct PreparedCache {
    pub(crate) progress: Vec<String>,
    pub(crate) commit_count: usize,
}

pub(crate) fn prepare_cache(
    repository: &git::Repository,
    rebuild: bool,
) -> Result<PreparedCache, AppError> {
    let (default_ref, tip) = repository.default_target()?;
    let shallow_boundaries = repository.shallow_boundaries()?;
    if !rebuild
        && let Some(commit_count) =
            cache::ready_commit_count(&repository.root, &default_ref, &tip, &shallow_boundaries)
    {
        return Ok(PreparedCache {
            progress: cache::shallow_warning(&shallow_boundaries)
                .into_iter()
                .collect(),
            commit_count,
        });
    }

    let snapshot = repository.read_default_history_at(default_ref, tip)?;
    let commit_count = snapshot.commits.len();
    let mut progress = vec!["Indexing local history...".to_owned()];
    if !snapshot.shallow_boundaries.is_empty() {
        progress
            .push("warning: local history is shallow; cache material is incomplete.".to_owned());
    }
    cache::publish(&repository.root, &snapshot)?;
    Ok(PreparedCache {
        progress,
        commit_count,
    })
}

pub(crate) fn execute(command: Command) -> Result<Outcome, AppError> {
    match command {
        Command::Index => index::run(),
        Command::Search { query, limit } => search::run(query, limit),
    }
}
