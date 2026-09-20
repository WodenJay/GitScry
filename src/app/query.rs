use std::collections::HashSet;

use crate::{
    analysis, cache,
    git::{HistoryTarget, Repository, WhyAnchor},
    render,
};

use super::{AppError, Outcome, prepare_cache, prepare_cache_at};

/// One query command: build the intent, prepare the cache, run the capability, render it.
pub(super) fn run(
    words: Vec<String>,
    paths: Vec<String>,
    limit: usize,
    capability: impl FnOnce(
        &rusqlite::Connection,
        &analysis::Intent,
        usize,
    ) -> Result<analysis::Report, AppError>,
) -> Result<Outcome, AppError> {
    let intent = analysis::Intent::parse(&words, &paths)?;
    let repository = Repository::discover()?;
    let prepared = prepare_cache(&repository)?;
    let connection = cache::open(&repository.root)?;
    let mut report = capability(&connection, &intent, limit)?;
    let mut progress = prepared.progress;
    progress.append(&mut report.warnings);
    Ok(Outcome {
        progress,
        message: render::format_report(&report),
        notices: report.notices.clone(),
    })
}

/// One path query: build the intent, prepare the cache, run the capability, render it.
pub(super) fn run_paths(
    paths: Vec<String>,
    limit: usize,
    capability: impl FnOnce(
        &rusqlite::Connection,
        &analysis::Intent,
        &[Vec<u8>],
        usize,
    ) -> Result<analysis::Report, AppError>,
) -> Result<Outcome, AppError> {
    let intent = analysis::Intent::paths(&paths)?;
    let repository = Repository::discover()?;
    let prepared = prepare_cache(&repository)?;
    let connection = cache::open(&repository.root)?;
    let worktree_paths = repository.worktree_paths()?;
    let mut report = capability(&connection, &intent, &worktree_paths, limit)?;
    let mut progress = prepared.progress;
    progress.append(&mut report.warnings);
    Ok(Outcome {
        progress,
        message: render::format_report(&report),
        notices: report.notices.clone(),
    })
}

pub(super) fn run_regression(
    words: Vec<String>,
    path: String,
    symbol: Option<String>,
    good: Option<String>,
    bad: String,
    limit: usize,
) -> Result<Outcome, AppError> {
    let intent = analysis::Intent::symptom(&words, &path)?;
    let repository = Repository::discover()?;
    let target =
        repository.pin_regression_target(&bad, good.as_deref(), &path, symbol.as_deref())?;
    let cached_target = repository.default_target_if_available()?;
    let (prepared, direct_history) = match cached_target {
        Some((default_ref, cached_tip)) => {
            let prepared = prepare_cache(&repository)?;
            let direct_history = if repository.is_ancestor(&target.bad_revision, &cached_tip)? {
                None
            } else {
                Some(repository.read_default_history_at(HistoryTarget {
                    default_ref,
                    tip: target.bad_revision.clone(),
                    object_format: repository.object_format()?,
                    shallow_boundaries: repository.shallow_boundaries()?,
                })?)
            };
            (Some(prepared), direct_history)
        }
        None => (
            None,
            Some(repository.read_default_history_at(HistoryTarget {
                default_ref: target.bad_revision.clone(),
                tip: target.bad_revision.clone(),
                object_format: repository.object_format()?,
                shallow_boundaries: repository.shallow_boundaries()?,
            })?),
        ),
    };
    let connection = match prepared.as_ref() {
        Some(_) => cache::open(&repository.root)?,
        None => cache::open_in_memory()?,
    };
    let mut report = analysis::regression(
        &connection,
        &intent,
        &target,
        direct_history.as_ref(),
        limit,
    )?;
    let mut progress = prepared
        .as_ref()
        .map(|cache| cache.progress.clone())
        .unwrap_or_else(|| {
            vec!["warning: no default branch; using target-specific history only.".to_owned()]
        });
    progress.append(&mut report.warnings);
    Ok(Outcome {
        progress,
        message: render::format_report(&report),
        notices: report.notices.clone(),
    })
}
pub(super) fn run_why(
    revision: String,
    path: String,
    anchor: WhyAnchor,
    limit: usize,
) -> Result<Outcome, AppError> {
    let repository = Repository::discover()?;
    let target = repository.pin_why_target(&revision, &path, anchor)?;
    let prepared = if repository.default_target_if_available()?.is_some() {
        prepare_cache(&repository)?
    } else {
        let report = analysis::why_without_cache(&target, limit)?;
        return Ok(Outcome {
            progress: vec![
                "warning: no default branch; using target-specific blame only.".to_owned(),
            ],
            message: render::format_report(&report),
            notices: report.notices.clone(),
        });
    };
    let connection = cache::open(&repository.root)?;
    let report = analysis::why(&connection, &target, limit)?;
    Ok(Outcome {
        progress: prepared.progress,
        message: render::format_report(&report),
        notices: report.notices.clone(),
    })
}

pub(super) fn run_trace_fix(
    revision: String,
    paths: Vec<String>,
    limit: usize,
) -> Result<Outcome, AppError> {
    let repository = Repository::discover()?;
    let target = repository.pin_trace_fix(&revision, &paths)?;
    let Some((default_ref, default_tip)) = repository.default_target_if_available()? else {
        let report = analysis::trace_fix_without_cache(&target, limit)?;
        return Ok(Outcome {
            progress: vec![
                "warning: no default branch; introducing candidates are unavailable.".to_owned(),
            ],
            message: render::format_report(&report),
            notices: report.notices.clone(),
        });
    };
    let prepared = prepare_cache_at(&repository, default_ref, default_tip.clone())?;
    let reachable = repository
        .reachable_commits(&default_tip)?
        .into_iter()
        .collect::<HashSet<_>>();
    let connection = cache::open(&repository.root)?;
    let report = analysis::trace_fix(&connection, &target, &reachable, limit)?;
    Ok(Outcome {
        progress: prepared.progress,
        message: render::format_report(&report),
        notices: report.notices.clone(),
    })
}
