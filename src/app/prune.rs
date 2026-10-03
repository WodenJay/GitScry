use crate::{cache, git};

use super::{AppError, Outcome};

pub(super) fn run(dry_run: bool) -> Result<Outcome, AppError> {
    let repository = git::Repository::discover()?;
    let (progress, prune_report) = cache::prune(&repository, dry_run)?;
    Ok(Outcome {
        progress,
        warnings: Vec::new(),
        message: String::new(),
        notices: Vec::new(),
        report: None,
        usage_report: None,
        github_links: None,
        clear_report: None,
        prune_report: Some(prune_report),
    })
}
