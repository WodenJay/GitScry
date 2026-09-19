use crate::{cache, git};

use super::{AppError, Outcome};

pub(super) fn run() -> Result<Outcome, AppError> {
    let repository = git::Repository::discover()?;
    let snapshot = repository.read_default_history()?;
    let commit_count = snapshot.commits.len();
    let mut progress = vec!["Indexing local history...".to_owned()];
    if !snapshot.shallow_boundaries.is_empty() {
        progress
            .push("warning: local history is shallow; cache material is incomplete.".to_owned());
    }
    cache::publish(&repository.root, &snapshot)?;

    Ok(Outcome {
        progress,
        message: format!(
            "Indexed {commit_count} commit{}.",
            if commit_count == 1 { "" } else { "s" }
        ),
    })
}
