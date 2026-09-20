use crate::{cache, git};

use super::{AppError, Outcome};

pub(super) fn run() -> Result<Outcome, AppError> {
    let repository = git::Repository::discover()?;
    let prepared = cache::prepare(&repository)?;

    Ok(Outcome {
        progress: prepared.progress,
        message: format!(
            "Indexed {} commit{}.",
            prepared.commit_count,
            if prepared.commit_count == 1 { "" } else { "s" }
        ),
        notices: Vec::new(),
    })
}
