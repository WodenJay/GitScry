use crate::analysis::patch;
use crate::analysis::query::{Context, Options, Outcome, QueryReport};
use crate::{
    analysis::{PatchExcerpt, SearchScopeInfo},
    cache::HistoryCommit,
};
use crate::{app::AppError, cache, git::Repository};
use std::collections::HashSet;

pub(in crate::analysis) fn execute(
    path: String,
    at: Option<String>,
    offset: usize,
    last: bool,
    options: Options,
) -> Result<Outcome, AppError> {
    let repository = Repository::discover()?;
    let session = cache::open_query(&repository.root)?;
    let revision = match at {
        Some(revision) => revision,
        None => session.completed_tip()?,
    };
    let target = repository.pin_timeline_target(&revision, &path)?;
    session.require_revision(&target.revision)?;
    let context = Context::for_target(session, options.scope, &target.revision)?;
    let reachable = context.session.ancestors(&target.revision)?;
    let eligible = context.eligible_revisions(&target.revision)?;
    let history = context.session.timeline_history(&target.path, &reachable)?;
    let mut report = Report::from_history(
        target.revision,
        target.path,
        history,
        eligible.as_ref(),
        options.limit,
        offset,
        last,
    );
    if options.patch {
        patch::attach_timeline_patch_excerpts(&context.session, &mut report)?;
    }
    Ok(context.finish(QueryReport::Timeline(report)))
}

pub(crate) struct Report {
    pub(crate) target_revision: String,
    pub(crate) path: Vec<u8>,
    pub(crate) patch_mode: bool,
    pub(crate) total: usize,
    pub(crate) scope: Option<SearchScopeInfo>,
    pub(crate) offset: usize,
    pub(crate) limit: usize,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) has_more: bool,
    pub(crate) entries: Vec<Entry>,
}

pub(crate) struct Entry {
    pub(crate) commit_id: String,
    pub(crate) change_ordinal: i64,
    pub(crate) timestamp: String,
    pub(crate) subject: String,
    pub(crate) path: Vec<u8>,
    pub(crate) change_type: &'static str,
    pub(crate) parent_count: usize,
    pub(crate) shallow_boundary: bool,
    pub(crate) patch: Option<PatchExcerpt>,
}

impl Report {
    pub(crate) fn from_history(
        target_revision: String,
        path: Vec<u8>,
        history: Vec<HistoryCommit>,
        eligible_revisions: Option<&HashSet<String>>,
        limit: usize,
        requested_offset: usize,
        last: bool,
    ) -> Self {
        let latest_origin = history
            .iter()
            .flat_map(|commit| {
                commit.changes.iter().filter_map(|change| {
                    let is_copy = change.status.starts_with('C');
                    // A merge's first-parent diff can say A for a file added by another parent.
                    let is_first_parent_addition =
                        change.status.starts_with('A') && commit.parent_count <= 1;
                    (commit.anchored_ordinals.contains(&change.ordinal)
                        && (is_copy || is_first_parent_addition))
                        .then_some(commit.position)
                })
            })
            .max();
        let mut entries = history
            .iter()
            .filter(|commit| latest_origin.is_none_or(|origin| commit.position >= origin))
            .filter(|commit| {
                eligible_revisions.is_none_or(|eligible| eligible.contains(&commit.oid))
            })
            .filter_map(|commit| {
                let change = commit
                    .changes
                    .iter()
                    .find(|change| commit.anchored_ordinals.contains(&change.ordinal))?;
                let historical_path = change
                    .new_path
                    .as_ref()
                    .or(change.old_path.as_ref())?
                    .clone();
                Some((
                    commit.position,
                    Entry {
                        commit_id: commit.oid.clone(),
                        change_ordinal: change.ordinal,
                        timestamp: format_timestamp(commit.commit_time),
                        subject: commit.subject.clone(),
                        path: historical_path,
                        change_type: if commit.shallow_boundary
                            && matches!(change.status.chars().next(), Some('A' | 'C'))
                        {
                            "unknown"
                        } else {
                            change_type(&change.status)
                        },
                        parent_count: commit.parent_count,
                        shallow_boundary: commit.shallow_boundary,
                        patch: None,
                    },
                ))
            })
            .collect::<Vec<_>>();
        entries.sort_by_key(|(position, _)| *position);
        entries.dedup_by(|(_, left), (_, right)| left.commit_id == right.commit_id);
        let all_entries = entries
            .into_iter()
            .map(|(_, entry)| entry)
            .collect::<Vec<_>>();

        let total = all_entries.len();
        let offset = if last {
            total.saturating_sub(limit)
        } else {
            requested_offset
        };
        let entries = all_entries
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect::<Vec<_>>();
        let start = if entries.is_empty() { 0 } else { offset + 1 };
        let end = if entries.is_empty() {
            0
        } else {
            offset.saturating_add(entries.len())
        };

        Self {
            target_revision,
            path,
            patch_mode: false,
            total,
            scope: None,
            offset,
            limit,
            start,
            end,
            has_more: offset.saturating_add(entries.len()) < total,
            entries,
        }
    }
}

fn change_type(status: &str) -> &'static str {
    match status.chars().next() {
        Some('A') => "added",
        Some('M' | 'T') => "modified",
        Some('D') => "deleted",
        Some('R') => "renamed",
        Some('C') => "copied",
        Some('U') => "unmerged",
        _ => "changed",
    }
}

pub(in crate::analysis) fn format_timestamp(timestamp: i64) -> String {
    let days = timestamp.div_euclid(86_400);
    let seconds = timestamp.rem_euclid(86_400);
    let (year, month, day) = civil_date(days);
    let hour = seconds / 3_600;
    let minute = seconds % 3_600 / 60;
    let second = seconds % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_date(days_since_epoch: i64) -> (i64, i64, i64) {
    let days = days_since_epoch + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}
