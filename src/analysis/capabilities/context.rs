//! Select path-association material; no content-similarity or review conclusions.
use super::super::{Citation, SearchScopeInfo, retrieval};
use super::relations::{is_test_path, relation_score};
use crate::{
    app::AppError,
    cache::{QuerySession, SearchFilter},
    git::{CurrentChange, current_regular_file},
};
use std::{collections::HashSet, path::Path};

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
    pub(crate) limitations: Vec<String>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Category {
    Test,
    CoChangingFile,
}
impl Category {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Test => "test",
            Self::CoChangingFile => "co_changing_file",
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
}

impl Report {
    pub(crate) fn empty(input: CurrentChange) -> Self {
        Self {
            input, cache_tip: None, scope: None, suggestions: Vec::new(), matched_count: 0,
            truncated: false, omitted_input_paths: 0, warnings: Vec::new(),
            limitations: vec![
                "Path associations only: no content similarity, mandatory edits, coverage verdicts, or required test execution.".to_owned(),
                "No file contents are analyzed; symlink targets and submodule contents are not read. Rename detection is not exhaustive.".to_owned(),
                "No results does not prove absence of related history outside the analyzed scope.".to_owned(),
            ],
        }
    }
}

pub(crate) fn run(
    session: &QuerySession,
    input: CurrentChange,
    root: &Path,
    limit: usize,
    scope: Option<&SearchFilter>,
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
    report.limitations.push("Only available published cache history is queried, not all repository or branch history. Merge and mass-change commits (over 50 paths) are excluded.".to_owned());
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
        let ubiquity = candidate.total_touches as f64 / history.eligible_commits as f64;
        let score = relation_score(
            count,
            proportion,
            ubiquity,
            candidate.seed_keys.len(),
            selected_paths.len(),
        );
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
            },
            supporting,
        ));
    }
    if omitted_tests {
        report.warnings.push("warning: historical test paths absent as safe regular files in the current worktree were omitted; renames are not resolved".to_owned());
    }
    ranked.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| (a.2.category != Category::Test).cmp(&(b.2.category != Category::Test)))
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.2.path.cmp(&b.2.path))
    });
    report.matched_count = ranked.len();
    let mut tests = 0;
    let mut files = 0;
    for (_, _, mut suggestion, supporting) in ranked {
        let category_count = if suggestion.category == Category::Test {
            &mut tests
        } else {
            &mut files
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
