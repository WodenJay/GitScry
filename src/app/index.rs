use crate::{cache, git};

use super::{AppError, IndexStage, Outcome};

pub(super) fn run(
    semantic: bool,
    no_semantic: bool,
    report: &mut dyn FnMut(IndexStage),
) -> Result<Outcome, AppError> {
    let repository = git::Repository::discover()?;
    let prepared = cache::prepare(&repository, report)?;
    let (progress, current_head_commit_count, semantic_enabled) = prepared.release();
    let preference = match (semantic, no_semantic) {
        (true, false) => cache::SemanticPreference::Enable,
        (false, true) => cache::SemanticPreference::Disable,
        (false, false) => cache::SemanticPreference::Preserve,
        (true, true) => unreachable!("clap prevents conflicting semantic options"),
    };
    if !matches!(preference, cache::SemanticPreference::Preserve) || semantic_enabled {
        cache::maintain_semantic(&repository.common_dir, preference, report)?;
    }
    report(IndexStage::Complete);

    Ok(Outcome {
        progress,
        warnings: Vec::new(),
        message: format!(
            "Indexed {current_head_commit_count} commit{} reachable from current HEAD.",
            if current_head_commit_count == 1 {
                ""
            } else {
                "s"
            }
        ),
        notices: Vec::new(),
        report: None,
        usage_report: None,
        github_links: None,
        clear_report: None,
        prune_report: None,
    })
}
