use crate::{cache, git};

use super::{AppError, IndexStage, Outcome};

pub(super) fn run(report: &mut dyn FnMut(IndexStage)) -> Result<Outcome, AppError> {
    let repository = git::Repository::discover()?;
    let prepared = cache::prepare(&repository, report)?;

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
