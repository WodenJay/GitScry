//! Aggregation over eligible touches, independent of display and lineage traversal.
use super::timeline::format_timestamp;
use crate::analysis::SearchScopeInfo;
use crate::analysis::query::{Context, Options, Outcome, QueryReport, scope};
use crate::{app::AppError, cache::FileTouches, git::Repository};

fn normalize_hotspot_path_prefix(prefix: Option<String>) -> Result<Option<String>, AppError> {
    let Some(prefix) = prefix else {
        return Ok(None);
    };
    let normalized = prefix.replace('\\', "/");
    let normalized = normalized.trim_end_matches('/');
    let bytes = normalized.as_bytes();
    let is_windows_drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if normalized.is_empty()
        || normalized.starts_with('/')
        || is_windows_drive
        || normalized.contains('\0')
        || normalized
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(AppError::input(
            "--path-prefix must be a repository-relative directory without `.` or `..` components",
        ));
    }
    Ok(Some(normalized.to_owned()))
}

pub(in crate::analysis) fn execute(
    options: Options,
    path_prefix: Option<String>,
) -> Result<Outcome, AppError> {
    let path_prefix = normalize_hotspot_path_prefix(path_prefix)?;
    let repository = Repository::discover()?;
    let requested = options.scope.to_rev.clone();
    let implicit_target = requested.is_none();
    let (context, target) = Context::prepare_target(
        &repository,
        requested.as_deref(),
        options.scope,
        |revision| repository.resolve_commit(revision),
    )?;
    let history_targets = if implicit_target {
        scope::cached_head_history_frontier(&context.session, &repository, &context.pinned_head)?
    } else {
        vec![target.clone()]
    };
    let eligible = context.eligible_revisions(&target)?;
    let paths = repository
        .tracked_files(&target)?
        .into_iter()
        .filter(|path| {
            path_prefix.as_ref().is_none_or(|prefix| {
                path.starts_with(prefix.as_bytes()) && path.get(prefix.len()) == Some(&b'/')
            })
        })
        .collect();
    let touches = context
        .session
        .hotspot_touches(&history_targets, paths, eligible.as_ref())?;
    let report = Report::aggregate(target, touches, options.limit, path_prefix);
    Ok(context.finish(QueryReport::Hotspots(report)))
}
pub(crate) struct Report {
    pub(crate) target: String,
    pub(crate) total: usize,
    pub(crate) scope: Option<SearchScopeInfo>,
    pub(crate) path_prefix: Option<String>,
    pub(crate) files: Vec<File>,
}
pub(crate) struct File {
    pub(crate) path: Vec<u8>,
    pub(crate) touching_commits: usize,
    pub(crate) last_changed: String,
    pub(crate) additions: Option<u64>,
    pub(crate) deletions: Option<u64>,
    pub(crate) churn_complete: bool,
}
impl Report {
    pub(crate) fn aggregate(
        target: String,
        touches: Vec<FileTouches>,
        limit: usize,
        path_prefix: Option<String>,
    ) -> Self {
        let mut files: Vec<_> = touches
            .into_iter()
            .filter_map(|touches| {
                let latest = *touches.times.iter().max()?;
                Some(File {
                    path: touches.path,
                    touching_commits: touches.times.len(),
                    last_changed: format_timestamp(latest),
                    additions: touches.additions,
                    deletions: touches.deletions,
                    churn_complete: touches.churn_complete,
                })
            })
            .collect();
        files.sort_by(|a, b| {
            b.touching_commits
                .cmp(&a.touching_commits)
                .then_with(|| a.path.cmp(&b.path))
        });
        let total = files.len();
        files.truncate(limit);
        Self {
            target,
            scope: None,
            path_prefix,
            total,
            files,
        }
    }
}
