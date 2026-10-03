//! Select path-association material; no content-similarity or review conclusions.
mod abandonment;
mod content;

use super::relations::is_test_path;
use crate::analysis::query::{Context, Options, Outcome, QueryReport, scope};
use crate::analysis::{Citation, SearchScopeInfo, retrieval};
use crate::{
    app::AppError,
    cache::{QuerySession, SearchFilter},
    git::{CurrentChange, Repository, current_regular_file},
};
use std::{collections::HashSet, path::Path};

pub(in crate::analysis) fn execute(
    staged: bool,
    hybrid: bool,
    options: Options,
) -> Result<Outcome, AppError> {
    let repository = Repository::discover()?;
    let input = repository.current_change(staged)?;
    if input.changes.is_empty() {
        scope::validate_time_bounds(&options.scope)?;
        let mut report = Report::empty(input);
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
        &context.session,
        input,
        &repository.root,
        options.limit,
        context.filter(),
        hybrid,
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
        }
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
impl Report {
    pub(crate) fn empty(input: CurrentChange) -> Self {
        Self {
            input, cache_tip: None, scope: None, suggestions: Vec::new(), matched_count: 0,
            truncated: false, omitted_input_paths: 0, warnings: Vec::new(),
            omitted_content_bases: 0, omitted_content_signals: 0,
            historical_content_truncated: false, omitted_historical_hunks: 0,
            semantic_requested: false, omitted_semantic_bases: 0,
            semantic_candidates: 0, semantic_content_truncated: false,
            limitations: vec![
                "Exact changed-code identity and path associations are context material, not a same-kind change conclusion, mandatory edits, coverage verdicts, or required test execution.".to_owned(),
                "Content is bounded to 512 local hunk sides, 24 signals per side, 20,000 historical hunks, 64 MiB decoded payloads, 256 KiB per historical hunk, 128 verified commits, and 16 excerpts per commit (240 bytes each). Symlink targets and submodule contents are not read. Rename detection is not exhaustive.".to_owned(),
                "No results does not prove absence of related history outside the analyzed scope.".to_owned(),
            ],
        }
    }
}

fn run(
    session: &QuerySession,
    input: CurrentChange,
    root: &Path,
    limit: usize,
    scope: Option<&SearchFilter>,
    hybrid: bool,
) -> Result<Report, AppError> {
    let paths = input.paths();
    let excluded = paths.iter().cloned().collect::<HashSet<_>>();
    let selected_paths = &paths[..paths.len().min(INPUT_PATH_LIMIT)];
    let history = session.exact_relation_history(selected_paths, 50, scope)?;
    let mut report = Report::empty(input);
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
        let score = candidate.seed_keys.len();
        let mut supporting = candidate.supporting;
        supporting.sort_by(|a, b| {
            b.commit_time
                .cmp(&a.commit_time)
                .then_with(|| a.oid.cmp(&b.oid))
        });
        let latest = supporting[0].commit_time;
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
        ranked.push((
            score,
            latest,
            Suggestion {
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
            },
            supporting,
        ));
    }
    if omitted_tests {
        report.warnings.push("warning: historical test paths absent as safe regular files in the current worktree were omitted; renames are not resolved".to_owned());
    }
    let changes = content::discover(session, &mut report, scope, hybrid)?;
    for (strength, time, suggestion) in abandonment::compose(session, changes, scope)? {
        ranked.push((strength, time, suggestion, Vec::new()));
    }
    ranked.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.2.category.cmp(&b.2.category))
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.2.path.cmp(&b.2.path))
            .then_with(|| {
                a.2.citations
                    .first()
                    .map(|c| &c.oid)
                    .cmp(&b.2.citations.first().map(|c| &c.oid))
            })
    });
    report.matched_count = ranked.len();
    let mut tests = 0;
    let mut files = 0;
    let mut changes = 0;
    let mut abandonments = 0;
    for (_, _, mut suggestion, supporting) in ranked {
        let category_count = match suggestion.category {
            Category::RecordedAbandonment => &mut abandonments,
            Category::Test => &mut tests,
            Category::CoChangingFile => &mut files,
            Category::HistoricalChange => &mut changes,
        };
        if report.suggestions.len() >= limit || *category_count >= CATEGORY_LIMIT {
            continue;
        }
        *category_count += 1;
        for support in supporting.into_iter().take(CITATION_LIMIT) {
            let subject = retrieval::commit_text(session, &support.oid)?
                .map(|(subject, _)| subject)
                .unwrap_or_default();
            suggestion
                .citations
                .push(Citation::new(support.oid, subject));
        }
        report.suggestions.push(suggestion);
    }
    report.truncated = report.suggestions.len() < report.matched_count;
    Ok(report)
}
