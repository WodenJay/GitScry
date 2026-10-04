//! Select path-association material; no content-similarity or review conclusions.
mod abandonment;
mod content;
mod historical_followup;

use super::relations::is_test_path;
use crate::analysis::query::{Context, Options, Outcome, QueryReport, scope};
use crate::analysis::{Citation, SearchScopeInfo, retrieval};
use crate::{
    app::AppError,
    git::{CurrentChange, Repository, current_regular_file},
};
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    path::Path,
};

#[derive(Clone, Copy)]
struct HistoricalFollowupOptions {
    enabled: bool,
    days: usize,
}
pub(in crate::analysis) fn execute(
    staged: bool,
    hybrid: bool,
    historical_followup_enabled: bool,
    historical_followup_days: usize,
    options: Options,
) -> Result<Outcome, AppError> {
    let repository = Repository::discover()?;
    let input = repository.current_change(staged)?;
    let historical_followup = HistoricalFollowupOptions {
        enabled: historical_followup_enabled,
        days: historical_followup_days,
    };
    if input.changes.is_empty() {
        scope::validate_time_bounds(&options.scope)?;
        let mut report = Report::empty(input, historical_followup);
        report.semantic_requested = hybrid;
        return Ok(Outcome {
            progress: Vec::new(),
            warnings: Vec::new(),
            report: QueryReport::Context(report),
        });
    }
    let context = Context::open(options.scope)?;
    if hybrid {
        context.session.require_semantic_ready(context.filter())?;
    }
    let report = run(
        &context,
        input,
        &repository,
        &repository.root,
        options.limit,
        hybrid,
        historical_followup,
    )?;
    Ok(context.finish(QueryReport::Context(report)))
}

const INPUT_PATH_LIMIT: usize = 256;
const CITATION_LIMIT: usize = 3;
const CATEGORY_LIMIT: usize = 3;

pub(crate) struct Report {
    pub(crate) input: CurrentChange,
    pub(crate) cache_tip: Option<String>,
    pub(crate) scope: Option<SearchScopeInfo>,
    pub(crate) suggestions: Vec<Suggestion>,
    pub(crate) matched_count: usize,
    pub(crate) truncated: bool,
    pub(crate) omitted_input_paths: usize,
    pub(crate) omitted_content_bases: usize,
    pub(crate) omitted_content_signals: usize,
    pub(crate) historical_content_truncated: bool,
    pub(crate) omitted_historical_hunks: usize,
    pub(crate) semantic_requested: bool,
    pub(crate) historical_followup_enabled: bool,
    pub(crate) historical_followup_days: usize,
    pub(crate) omitted_semantic_bases: usize,
    pub(crate) semantic_candidates: usize,
    pub(crate) semantic_content_truncated: bool,
    pub(crate) limitations: Vec<String>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Category {
    RecordedAbandonment,
    HistoricalChange,
    HistoricalFollowup,
    Test,
    CoChangingFile,
}
impl Category {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::RecordedAbandonment => "recorded_abandonment",
            Self::Test => "test",
            Self::CoChangingFile => "co_changing_file",
            Self::HistoricalChange => "historical_change",
            Self::HistoricalFollowup => "historical_followup",
        }
    }
    fn index(self) -> usize {
        match self {
            Self::RecordedAbandonment => 0,
            Self::HistoricalChange => 1,
            Self::HistoricalFollowup => 2,
            Self::Test => 3,
            Self::CoChangingFile => 4,
        }
    }
}

#[derive(Clone)]
pub(crate) struct HistoricalFollowupChain {
    pub(crate) origin_oid: String,
    pub(crate) later_oid: String,
}

pub(crate) struct CoChange {
    pub(crate) category: Category,
    pub(crate) associated_current_paths: Vec<Vec<u8>>,
    pub(crate) basis: Vec<String>,
    pub(crate) selection_routes: Vec<&'static str>,
    pub(crate) citations: Vec<Citation>,
    pub(crate) supporting_count: usize,
    pub(crate) citations_truncated: bool,
}
pub(crate) struct HistoricalFollowup {
    pub(crate) supporting_origins: usize,
    pub(crate) complete_origins: usize,
    pub(crate) independent_chains: usize,
    pub(crate) baseline_occurrences: usize,
    pub(crate) baseline_sample_size: usize,
    pub(crate) undisplayed_supporting_origins: usize,
    pub(crate) chains: Vec<HistoricalFollowupChain>,
    pub(crate) observation_days: usize,
    pub(crate) basis: Vec<String>,
}

impl HistoricalFollowup {
    pub(crate) fn support_proportion(&self) -> f64 {
        self.supporting_origins as f64 / self.complete_origins as f64
    }

    pub(crate) fn baseline_lift(&self) -> Option<f64> {
        if self.baseline_occurrences == 0
            || self.baseline_sample_size == 0
            || self.complete_origins == 0
        {
            return None;
        }
        let support = self.support_proportion();
        let baseline = self.baseline_occurrences as f64 / self.baseline_sample_size as f64;
        Some(support / baseline)
    }
}

pub(crate) struct Suggestion {
    pub(crate) category: Category,
    pub(crate) path: Vec<u8>,
    pub(crate) associated_current_paths: Vec<Vec<u8>>,
    pub(crate) basis: Vec<String>,
    pub(crate) selection_routes: Vec<&'static str>,
    pub(crate) citations: Vec<Citation>,
    pub(crate) supporting_count: usize,
    pub(crate) citations_truncated: bool,
    pub(crate) content_matches: Vec<ContentMatch>,
    pub(crate) content_matches_truncated: bool,
    pub(crate) abandonment: Option<crate::analysis::Failure>,
    pub(crate) historical_followup: Option<HistoricalFollowup>,
    pub(crate) co_change: Option<CoChange>,
}

pub(crate) struct ContentMatch {
    pub(crate) current_path: Vec<u8>,
    pub(crate) current_added: bool,
    pub(crate) current_old_start: usize,
    pub(crate) current_new_start: usize,
    pub(crate) historical_oid: String,
    pub(crate) historical_path: Vec<u8>,
    pub(crate) historical_added: bool,
    pub(crate) historical_line: usize,
    pub(crate) historical_old_start: i64,
    pub(crate) historical_new_start: i64,
    pub(crate) signals: Vec<String>,
    pub(crate) excerpt: Vec<u8>,
    pub(crate) excerpt_truncated: bool,
}

fn compare_ranked_suggestions(a: &RankedSuggestion, b: &RankedSuggestion) -> Ordering {
    b.strength
        .cmp(&a.strength)
        .then_with(|| a.suggestion.category.cmp(&b.suggestion.category))
        .then_with(|| {
            match (
                a.suggestion.historical_followup.as_ref(),
                b.suggestion.historical_followup.as_ref(),
            ) {
                (Some(left), Some(right)) => compare_followup_rank(left, right),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            }
        })
        .then_with(|| b.time.cmp(&a.time))
        .then_with(|| a.suggestion.path.cmp(&b.suggestion.path))
        .then_with(|| {
            a.suggestion
                .citations
                .first()
                .map(|citation| &citation.oid)
                .cmp(&b.suggestion.citations.first().map(|citation| &citation.oid))
        })
}
impl Report {
    fn empty(input: CurrentChange, historical_followup: HistoricalFollowupOptions) -> Self {
        Self {
            input, cache_tip: None, scope: None, suggestions: Vec::new(), matched_count: 0,
            truncated: false, omitted_input_paths: 0, warnings: Vec::new(),
            omitted_content_bases: 0, omitted_content_signals: 0,
            historical_content_truncated: false, omitted_historical_hunks: 0,
            semantic_requested: false,
            historical_followup_enabled: historical_followup.enabled,
            historical_followup_days: historical_followup.days,
            omitted_semantic_bases: 0,
            semantic_candidates: 0, semantic_content_truncated: false,
            limitations: vec![
                "Exact changed-code identity and path associations are context material, not a same-kind change conclusion, mandatory edits, coverage verdicts, or required test execution.".to_owned(),
                "Content is bounded to 512 local hunk sides, 24 signals per side, 20,000 historical hunks, 64 MiB decoded payloads, 256 KiB per historical hunk, 128 verified commits, and 16 excerpts per commit (240 bytes each). Symlink targets and submodule contents are not read. Rename detection is not exhaustive.".to_owned(),
                "No results does not prove absence of related history outside the analyzed scope.".to_owned(),
            ],
        }
    }
}

struct RankedSuggestion {
    strength: usize,
    time: i64,
    suggestion: Suggestion,
    supporting: Vec<String>,
    co_change_supporting: Vec<String>,
}

fn run(
    context: &Context,
    input: CurrentChange,
    repository: &Repository,
    root: &Path,
    limit: usize,
    hybrid: bool,
    historical_followup: HistoricalFollowupOptions,
) -> Result<Report, AppError> {
    let session = &context.session;
    let scope = context.filter();
    let paths = input.paths();
    let excluded = paths.iter().cloned().collect::<HashSet<_>>();
    let selected_paths = &paths[..paths.len().min(INPUT_PATH_LIMIT)];
    let history = session.exact_relation_history(selected_paths, 50, scope)?;
    let mut report = Report::empty(input, historical_followup);
    report.cache_tip = Some(session.completed_tip()?);
    if let Some(head) = &report.input.head
        && !session.contains_revision(head)?
    {
        report.warnings.push("warning: current HEAD is outside the published cache; associations use available cached history only".to_owned());
    }
    report.omitted_input_paths = paths.len().saturating_sub(INPUT_PATH_LIMIT);
    report.limitations.push("Only available published cache history is queried, not all repository or branch history. Path associations exclude merge and mass-change commits (over 50 paths); changed-code matching inspects bounded cached hunks independently.".to_owned());
    if report.omitted_input_paths > 0 {
        report.limitations.push(format!("Historical retrieval uses the first {INPUT_PATH_LIMIT} byte-sorted current paths; {} remaining paths retain input/exclusion identity but are not queried.", report.omitted_input_paths));
    }
    if history.mass_changes_filtered {
        report
            .warnings
            .push("warning: mass-change commits touching selected paths were excluded".to_owned());
    }
    let mut ranked = Vec::new();
    let mut omitted_tests = false;
    for candidate in history.candidates.into_values() {
        if excluded.contains(&candidate.path) {
            continue;
        }
        let count = candidate.supporting.len();
        if count == 0 {
            continue;
        }
        let proportion = count as f64 / history.seed_touch_commits as f64;
        // Same association-strength gate as medium/high focused path material.
        if count < 2 && proportion < 0.1 {
            continue;
        }
        let test = is_test_path(&candidate.path);
        if test && !current_regular_file(root, &candidate.path) {
            omitted_tests = true;
            continue;
        }
        let category = if test {
            Category::Test
        } else {
            Category::CoChangingFile
        };
        // Cross-route strength is distinct current-path support, not raw scores.
        let strength = candidate.seed_keys.len();
        let mut supporting = candidate.supporting;
        supporting.sort_by(|a, b| {
            b.commit_time
                .cmp(&a.commit_time)
                .then_with(|| a.oid.cmp(&b.oid))
        });
        let time = supporting[0].commit_time;
        let supporting = supporting.into_iter().map(|support| support.oid).collect();
        let mut associated = candidate.seed_keys.into_iter().collect::<Vec<_>>();
        associated.sort();
        let mut basis = vec![
            format!("co-change count {count}"),
            format!(
                "candidate proportion of seed touches {:.1}%",
                proportion * 100.0
            ),
            format!(
                "co-changed with {} selected current paths",
                associated.len()
            ),
        ];
        let mut routes = vec!["co_change"];
        if test {
            routes.push("test_path");
            basis.push(
                "test-shaped path exists as a regular file in the current worktree".to_owned(),
            );
        }
        ranked.push(RankedSuggestion {
            strength,
            time,
            suggestion: Suggestion {
                category,
                path: candidate.path,
                associated_current_paths: associated,
                basis,
                selection_routes: routes,
                citations: Vec::new(),
                supporting_count: count,
                citations_truncated: count > CITATION_LIMIT,
                content_matches: Vec::new(),
                content_matches_truncated: false,
                abandonment: None,
                historical_followup: None,
                co_change: None,
            },
            supporting,
            co_change_supporting: Vec::new(),
        });
    }
    if omitted_tests {
        report.warnings.push("warning: historical test paths absent as safe regular files in the current worktree were omitted; renames are not resolved".to_owned());
    }
    let changes = content::discover(session, &mut report, scope, hybrid)?;
    if historical_followup.enabled {
        for (strength, time, suggestion) in historical_followup::discover(
            session,
            repository,
            root,
            &report.input,
            &changes,
            historical_followup.days,
            scope,
        )? {
            ranked.push(RankedSuggestion {
                strength,
                time,
                suggestion,
                supporting: Vec::new(),
                co_change_supporting: Vec::new(),
            });
        }
    }
    for (strength, time, suggestion) in abandonment::compose(session, changes, scope)? {
        ranked.push(RankedSuggestion {
            strength,
            time,
            suggestion,
            supporting: Vec::new(),
            co_change_supporting: Vec::new(),
        });
    }
    merge_cochange_followups(&mut ranked);
    ranked.sort_by(compare_ranked_suggestions);
    report.matched_count = ranked.len();
    let mut category_counts = [0; 5];
    for mut ranked_suggestion in ranked {
        let category = ranked_suggestion.suggestion.category;
        let co_change_category = ranked_suggestion
            .suggestion
            .co_change
            .as_ref()
            .map(|co_change| co_change.category);
        if report.suggestions.len() >= limit
            || category_counts[category.index()] >= CATEGORY_LIMIT
            || co_change_category
                .is_some_and(|category| category_counts[category.index()] >= CATEGORY_LIMIT)
        {
            continue;
        }
        category_counts[category.index()] += 1;
        if let Some(co_change_category) = co_change_category
            && co_change_category != category
        {
            category_counts[co_change_category.index()] += 1;
        }
        for oid in ranked_suggestion
            .supporting
            .into_iter()
            .take(CITATION_LIMIT)
        {
            let subject = retrieval::commit_text(session, &oid)?
                .map(|(subject, _)| subject)
                .unwrap_or_default();
            ranked_suggestion
                .suggestion
                .citations
                .push(Citation::new(oid, subject));
        }
        if let Some(co_change) = &mut ranked_suggestion.suggestion.co_change {
            for oid in ranked_suggestion
                .co_change_supporting
                .into_iter()
                .take(CITATION_LIMIT)
            {
                let subject = retrieval::commit_text(session, &oid)?
                    .map(|(subject, _)| subject)
                    .unwrap_or_default();
                co_change.citations.push(Citation::new(oid, subject));
            }
        }
        report.suggestions.push(ranked_suggestion.suggestion);
    }
    report.truncated = report.suggestions.len() < report.matched_count;
    Ok(report)
}

fn merge_cochange_followups(ranked: &mut Vec<RankedSuggestion>) {
    let cochanges = ranked
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            matches!(
                item.suggestion.category,
                Category::Test | Category::CoChangingFile
            )
        })
        .map(|(index, item)| (item.suggestion.path.clone(), index))
        .collect::<HashMap<_, _>>();
    let mut merged_indices = HashSet::new();
    for followup_index in 0..ranked.len() {
        if ranked[followup_index]
            .suggestion
            .historical_followup
            .is_none()
        {
            continue;
        }
        let path = &ranked[followup_index].suggestion.path;
        let Some(&cochange_index) = cochanges.get(path) else {
            continue;
        };
        if followup_index < cochange_index {
            let (before, after) = ranked.split_at_mut(cochange_index);
            merge_ranked_cochange(&mut before[followup_index], &mut after[0]);
        } else {
            let (before, after) = ranked.split_at_mut(followup_index);
            merge_ranked_cochange(&mut after[0], &mut before[cochange_index]);
        }
        merged_indices.insert(cochange_index);
    }
    let mut index = 0;
    ranked.retain(|_| {
        let keep = !merged_indices.contains(&index);
        index += 1;
        keep
    });
}

fn merge_ranked_cochange(followup: &mut RankedSuggestion, cochange: &mut RankedSuggestion) {
    let suggestion = &mut cochange.suggestion;
    let cochange_routes = std::mem::take(&mut suggestion.selection_routes);
    for route in &cochange_routes {
        if !followup.suggestion.selection_routes.contains(route) {
            followup.suggestion.selection_routes.push(route);
        }
    }
    followup.suggestion.co_change = Some(CoChange {
        category: suggestion.category,
        associated_current_paths: std::mem::take(&mut suggestion.associated_current_paths),
        basis: std::mem::take(&mut suggestion.basis),
        selection_routes: cochange_routes,
        citations: std::mem::take(&mut suggestion.citations),
        supporting_count: suggestion.supporting_count,
        citations_truncated: suggestion.citations_truncated,
    });
    followup.co_change_supporting = std::mem::take(&mut cochange.supporting);
}

fn compare_followup_rank(left: &HistoricalFollowup, right: &HistoricalFollowup) -> Ordering {
    right
        .independent_chains
        .cmp(&left.independent_chains)
        .then_with(|| {
            compare_fractions(
                right.supporting_origins as u128,
                right.complete_origins as u128,
                left.supporting_origins as u128,
                left.complete_origins as u128,
            )
        })
        .then_with(|| {
            match (
                left.baseline_occurrences == 0,
                right.baseline_occurrences == 0,
            ) {
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                (true, true) => Ordering::Equal,
                (false, false) => compare_fractions(
                    right.supporting_origins as u128 * right.baseline_sample_size as u128,
                    right.complete_origins as u128 * right.baseline_occurrences as u128,
                    left.supporting_origins as u128 * left.baseline_sample_size as u128,
                    left.complete_origins as u128 * left.baseline_occurrences as u128,
                ),
            }
        })
}

fn compare_fractions(
    mut left_numerator: u128,
    mut left_denominator: u128,
    mut right_numerator: u128,
    mut right_denominator: u128,
) -> Ordering {
    let mut reversed = false;
    loop {
        let order = (left_numerator / left_denominator).cmp(&(right_numerator / right_denominator));
        if order != Ordering::Equal {
            return if reversed { order.reverse() } else { order };
        }
        let left_remainder = left_numerator % left_denominator;
        let right_remainder = right_numerator % right_denominator;
        if left_remainder == 0 || right_remainder == 0 {
            let order = match (left_remainder == 0, right_remainder == 0) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                (false, false) => unreachable!(),
            };
            return if reversed { order.reverse() } else { order };
        }
        left_numerator = left_denominator;
        left_denominator = left_remainder;
        right_numerator = right_denominator;
        right_denominator = right_remainder;
        reversed = !reversed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranked_suggestion(
        category: Category,
        path: &str,
        strength: usize,
        historical_followup: Option<HistoricalFollowup>,
    ) -> RankedSuggestion {
        RankedSuggestion {
            strength,
            time: 0,
            suggestion: Suggestion {
                category,
                path: path.as_bytes().to_vec(),
                associated_current_paths: Vec::new(),
                basis: Vec::new(),
                selection_routes: Vec::new(),
                citations: Vec::new(),
                supporting_count: 0,
                citations_truncated: false,
                content_matches: Vec::new(),
                content_matches_truncated: false,
                abandonment: None,
                historical_followup,
                co_change: None,
            },
            supporting: Vec::new(),
            co_change_supporting: Vec::new(),
        }
    }
    fn followup(
        independent_chains: usize,
        supporting_origins: usize,
        complete_origins: usize,
        baseline_occurrences: usize,
        baseline_sample_size: usize,
    ) -> HistoricalFollowup {
        HistoricalFollowup {
            supporting_origins,
            complete_origins,
            independent_chains,
            baseline_occurrences,
            baseline_sample_size,
            undisplayed_supporting_origins: 0,
            chains: Vec::new(),
            observation_days: 7,
            basis: Vec::new(),
        }
    }

    #[test]
    fn ranked_context_order_is_transitive_between_followups_and_other_categories() {
        let weaker_followup = ranked_suggestion(
            Category::HistoricalFollowup,
            "weaker.rs",
            3,
            Some(followup(2, 3, 6, 0, 6)),
        );
        let ordinary = ranked_suggestion(Category::Test, "ordinary.rs", 3, None);
        let stronger_followup = ranked_suggestion(
            Category::HistoricalFollowup,
            "stronger.rs",
            2,
            Some(followup(2, 2, 2, 0, 2)),
        );

        assert_eq!(
            compare_ranked_suggestions(&weaker_followup, &ordinary),
            Ordering::Less
        );
        assert_eq!(
            compare_ranked_suggestions(&ordinary, &stronger_followup),
            Ordering::Less
        );
        assert_eq!(
            compare_ranked_suggestions(&weaker_followup, &stronger_followup),
            Ordering::Less
        );

        let lower_proportion = ranked_suggestion(
            Category::HistoricalFollowup,
            "lower.rs",
            2,
            Some(followup(2, 3, 6, 0, 6)),
        );
        let higher_proportion = ranked_suggestion(
            Category::HistoricalFollowup,
            "higher.rs",
            2,
            Some(followup(2, 2, 2, 0, 2)),
        );
        assert_eq!(
            compare_ranked_suggestions(&higher_proportion, &lower_proportion),
            Ordering::Less
        );
    }

    #[test]
    fn historical_followups_rank_by_chains_support_then_baseline_lift() {
        assert_eq!(
            compare_followup_rank(&followup(3, 1, 2, 0, 4), &followup(2, 2, 2, 1, 4)),
            Ordering::Less
        );
        assert_eq!(
            compare_followup_rank(&followup(2, 2, 3, 1, 3), &followup(2, 2, 4, 0, 4)),
            Ordering::Less
        );
        assert_eq!(
            compare_followup_rank(&followup(2, 2, 4, 1, 4), &followup(2, 2, 4, 2, 4)),
            Ordering::Less
        );

        let zero_baseline = followup(2, 2, 4, 0, 4);
        assert_eq!(zero_baseline.baseline_lift(), None);
        assert_eq!(
            compare_followup_rank(&zero_baseline, &followup(2, 2, 4, 1, 4)),
            Ordering::Less
        );
    }
}
