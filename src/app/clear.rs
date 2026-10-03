use crate::{cache, git};

use super::{AppError, Outcome};

pub(super) fn run(dry_run: bool) -> Result<Outcome, AppError> {
    let repository = git::Repository::discover()?;
    let (progress, clear_report) = cache::clear(&repository, dry_run)?;
    Ok(Outcome {
        progress,
        warnings: Vec::new(),
        message: String::new(),
        notices: Vec::new(),
        report: None,
        usage_report: None,
        github_links: None,
        clear_report: Some(clear_report),
        index_report: None,
    })
}
