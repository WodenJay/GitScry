use crate::git;

use super::{AppError, Outcome, prepare_cache};

pub(super) fn run() -> Result<Outcome, AppError> {
    let repository = git::Repository::discover()?;
    let prepared = prepare_cache(&repository)?;

    Ok(Outcome {
        progress: prepared.progress,
        message: format!(
            "Indexed {} commit{}.",
            prepared.commit_count,
            if prepared.commit_count == 1 { "" } else { "s" }
        ),
    })
}
