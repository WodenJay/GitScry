use crate::{cache, git};

use super::{AppError, IndexReport, IndexStage, Outcome};

pub(super) fn run(
    semantic: bool,
    no_semantic: bool,
    report: &mut dyn FnMut(IndexStage),
) -> Result<Outcome, AppError> {
    let repository = git::Repository::discover()?;
    let prepared = cache::prepare(&repository, report)?;
    let (progress, current_head_commit_count, semantic_enabled, pinned_tip) = prepared.release();
    let preference = match (semantic, no_semantic) {
        (true, false) => cache::SemanticPreference::Enable,
        (false, true) => cache::SemanticPreference::Disable,
        (false, false) => cache::SemanticPreference::Preserve,
        (true, true) => unreachable!("clap prevents conflicting semantic options"),
    };
    if !matches!(preference, cache::SemanticPreference::Preserve) || semantic_enabled {
        cache::maintain_semantic(&repository, &pinned_tip, preference, report)?;
    }
    report(IndexStage::Complete);

    Ok(Outcome {
        progress,
        warnings: Vec::new(),
        message: String::new(),
        notices: Vec::new(),
        report: None,
        usage_report: None,
        github_links: None,
        clear_report: None,
        index_report: Some(IndexReport {
            current_head_commit_count,
            semantic_disabled: no_semantic,
        }),
        prune_report: None,
    })
}
