use super::{AppError, Outcome};
use crate::{
    analysis, cache,
    git::{Repository, WhyAnchor},
    render,
};
use std::path::Path;

/// One query command: build the intent, open the published cache, run the capability, render it.
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
    let session = cache::open_query()?;
    let mut report = capability(session.connection(), &intent, limit)?;
    let mut progress = session.progress().to_vec();
    progress.append(&mut report.warnings);
    Ok(Outcome {
        progress,
        message: render::format_report(&report),
        notices: report.notices.clone(),
    })
}

/// One path query: build the intent, open the published cache, run the capability, render it.
pub(super) fn run_paths(
    paths: Vec<String>,
    limit: usize,
    capability: impl FnOnce(
        &rusqlite::Connection,
        &analysis::Intent,
        &Path,
        usize,
    ) -> Result<analysis::Report, AppError>,
) -> Result<Outcome, AppError> {
    let intent = analysis::Intent::paths(&paths)?;
    let session = cache::open_query()?;
    let mut report = capability(session.connection(), &intent, session.root(), limit)?;
    let mut progress = session.progress().to_vec();
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
    let session = cache::open_query()?;
    session.require_revision(&target.bad_revision)?;
    if let Some(good_revision) = &target.good_revision {
        session.require_revision(good_revision)?;
    }
    let bad_reachable = analysis::ancestors(session.connection(), &target.bad_revision)?;
    let reachable = if let Some(good_revision) = &target.good_revision {
        let good_reachable = analysis::ancestors(session.connection(), good_revision)?;
        bad_reachable.difference(&good_reachable).cloned().collect()
    } else {
        bad_reachable
    };
    let mut report =
        analysis::regression(session.connection(), &intent, &target, &reachable, limit)?;
    let mut progress = session.progress().to_vec();
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
    let session = cache::open_query()?;
    session.require_revision(&target.revision)?;
    let reachable = analysis::ancestors(session.connection(), &target.revision)?;
    let mut report = analysis::why(session.connection(), &target, &reachable, limit)?;
    let mut progress = session.progress().to_vec();
    progress.append(&mut report.warnings);
    Ok(Outcome {
        progress,
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
    let session = cache::open_query()?;
    session.require_revision(&target.revision)?;
    let reachable = analysis::ancestors(session.connection(), &target.revision)?;
    let mut report = analysis::trace_fix(session.connection(), &target, &reachable, limit)?;
    let mut progress = session.progress().to_vec();
    progress.append(&mut report.warnings);
    Ok(Outcome {
        progress,
        message: render::format_report(&report),
        notices: report.notices.clone(),
    })
}
