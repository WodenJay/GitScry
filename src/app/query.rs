use super::{AppError, Outcome, QueryReport};
use crate::{
    analysis, cache,
    git::{Repository, WhyAnchor},
    render, timeline,
};
use std::path::Path;

fn query_outcome(session: &cache::QuerySession, report: analysis::Report) -> Outcome {
    let progress = session.progress().to_vec();
    let warnings = session.warnings().to_vec();
    let report = QueryReport::Analysis(report);
    let message = render::format_report(&report);
    let notices = report.notices().to_vec();
    Outcome {
        progress,
        warnings,
        message,
        notices,
        report: Some(report),
    }
}
/// One query command: build the intent, open the published cache, run the capability, render it.
pub(super) fn run(
    words: Vec<String>,
    paths: Vec<String>,
    limit: usize,
    capability: impl FnOnce(
        &cache::QuerySession,
        &analysis::Intent,
        usize,
    ) -> Result<analysis::Report, AppError>,
) -> Result<Outcome, AppError> {
    let intent = analysis::Intent::parse(&words, &paths)?;
    let session = cache::open_query()?;
    let report = capability(&session, &intent, limit)?;
    Ok(query_outcome(&session, report))
}

pub(super) fn run_with_patch(
    words: Vec<String>,
    paths: Vec<String>,
    limit: usize,
    path_only: bool,
    capability: impl FnOnce(
        &cache::QuerySession,
        &analysis::Intent,
        usize,
    ) -> Result<analysis::Report, AppError>,
) -> Result<Outcome, AppError> {
    let intent = analysis::Intent::parse(&words, &paths)?;
    let session = cache::open_query()?;
    let mut report = capability(&session, &intent, limit)?;
    analysis::attach_patch_excerpts(&session, &intent, &mut report, path_only, &paths)?;
    Ok(query_outcome(&session, report))
}

/// Run a literal changed-code query without tokenizing its text as a topic.
pub(super) fn run_code(
    query: String,
    path: Option<String>,
    direction: Option<analysis::CodeDirection>,
    limit: usize,
) -> Result<Outcome, AppError> {
    let session = cache::open_query()?;
    let report = analysis::code_search(&session, &query, path.as_deref(), direction, limit)?;
    Ok(query_outcome(&session, report))
}

/// One path query: build the intent, open the published cache, run the capability, render it.
pub(super) fn run_paths(
    paths: Vec<String>,
    limit: usize,
    include_all_citations: bool,
    capability: impl FnOnce(
        &cache::QuerySession,
        &analysis::Intent,
        &Path,
        usize,
        bool,
    ) -> Result<analysis::Report, AppError>,
) -> Result<Outcome, AppError> {
    let intent = analysis::Intent::paths(&paths)?;
    let session = cache::open_query()?;
    let report = capability(
        &session,
        &intent,
        session.root(),
        limit,
        include_all_citations,
    )?;
    Ok(query_outcome(&session, report))
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
    let bad_reachable = session.ancestors(&target.bad_revision)?;
    let reachable = if let Some(good_revision) = &target.good_revision {
        let good_reachable = session.ancestors(good_revision)?;
        bad_reachable.difference(&good_reachable).cloned().collect()
    } else {
        bad_reachable
    };
    let report = analysis::regression(&session, &intent, &target, &reachable, limit)?;
    Ok(query_outcome(&session, report))
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
    let reachable = session.ancestors(&target.revision)?;
    let report = analysis::why(&session, &target, &reachable, limit)?;
    Ok(query_outcome(&session, report))
}

pub(super) fn run_timeline(
    path: String,
    at: Option<String>,
    limit: usize,
    offset: usize,
    last: bool,
) -> Result<Outcome, AppError> {
    let session = cache::open_query()?;
    let revision = match at {
        Some(revision) => revision,
        None => session.completed_tip()?,
    };
    let repository = Repository::discover()?;
    let target = repository.pin_timeline_target(&revision, &path)?;
    session.require_revision(&target.revision)?;
    let reachable = session.ancestors(&target.revision)?;
    let history = session.timeline_history(&target.path, &reachable)?;
    let report =
        timeline::Report::from_history(target.revision, target.path, history, limit, offset, last);
    let progress = session.progress().to_vec();
    let warnings = session.warnings().to_vec();
    let report = QueryReport::Timeline(report);
    let message = render::format_report(&report);
    Ok(Outcome {
        progress,
        warnings,
        message,
        notices: Vec::new(),
        report: Some(report),
    })
}

pub(super) fn run_trace_fix(
    revision: String,
    paths: Vec<String>,
    limit: usize,
    patch: bool,
) -> Result<Outcome, AppError> {
    let repository = Repository::discover()?;
    let target = repository.pin_trace_fix(&revision, &paths)?;
    let session = cache::open_query()?;
    session.require_revision(&target.revision)?;
    let reachable = session.ancestors(&target.revision)?;
    let mut report = analysis::trace_fix(&session, &target, &reachable, limit)?;
    if patch {
        analysis::attach_trace_fix_patch_excerpts(&session, &mut report)?;
    }
    Ok(query_outcome(&session, report))
}
