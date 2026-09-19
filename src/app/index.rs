use crate::{cache, git};

use super::{AppError, Outcome};

pub(super) fn run() -> Result<Outcome, AppError> {
    let repository = git::Repository::discover()?;
    let snapshot = repository.read_default_history()?;
    let commit_count = snapshot.commits.len();
    cache::publish(&repository.root, &snapshot)?;

    Ok(Outcome {
        progress: vec!["Indexing local history...".to_owned()],
        message: format!(
            "Indexed {commit_count} commit{}.",
            if commit_count == 1 { "" } else { "s" }
        ),
    })
}
