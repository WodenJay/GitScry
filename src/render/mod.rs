//! The only place GitScry writes to stdout and stderr.
//!
//! One reason to change: user-visible output formats. Process progress and warnings stay on
//! stderr; query reports go to stdout as either human-readable English or schema-versioned JSON.

mod clear;
mod conflicts;
mod context;
mod escape;
mod fate;
mod followups;
mod fragment_search;
mod github_links;
mod json;
mod material;
mod output;
mod prune;

mod hotspots;
mod patch_search;
mod patterns;
mod propagation;
mod timeline;
mod trace_removal;
mod usage;
use crate::{
    analysis::query::QueryReport,
    app::{self, AppError, IndexStage, Outcome, Progress, UpdateStage},
    cli::Command,
};
use std::io::{self, IsTerminal, Write};

fn format_report(
    report: &QueryReport,
    github_links: Option<&crate::github::LinksReport>,
) -> String {
    match report {
        QueryReport::PatchSearch(report) => patch_search::format_report(report),
        QueryReport::FragmentSearch(report) => fragment_search::format_report(report, github_links),
        QueryReport::Conflicts(report) => conflicts::format_report(report),
        QueryReport::Followups(report) => followups::format_report(report),
        QueryReport::Patterns(report) => patterns::format_report(report),
        QueryReport::Hotspots(report) => hotspots::format_report(report),
        QueryReport::Propagation(report) => propagation::format_report(report),
        QueryReport::Context(report) => context::format_report(report),
        QueryReport::TraceRemoval(report) => trace_removal::format_report(report),
        QueryReport::TraceRemovalFragment(report) => trace_removal::format_fragment_report(report),
        QueryReport::Analysis(report) => {
            let mut output = material::format_report(report);
            if let Some(versions) = &report.fix_versions {
                output.push_str("\n\nEquivalent-version scope: all local and fetched remote branch tips, independent of trace-fix historical scope and filters.\n");
                output.push_str(&patch_search::format_trace_fix_versions(versions));
            }
            if let Some(github_links) = github_links {
                output.push_str("\n\n");
                output.push_str(&github_links::format(github_links));
            }
            output
        }
        QueryReport::Timeline(report) => {
            let mut output = timeline::format_report(report);
            if let Some(github_links) = github_links {
                output.push_str("\n\n");
                output.push_str(&github_links::format(github_links));
            }
            output
        }
        QueryReport::Fate(report) => fate::format_report(report),
    }
}

fn format_index_report(report: &app::IndexReport) -> String {
    let count = report.selected_commit_count;
    let mut message = if report.selected.is_empty() {
        format!(
            "Indexed {count} commit{} reachable from current HEAD.",
            if count == 1 { "" } else { "s" }
        )
    } else {
        let mut message = format!(
            "Indexed {count} commit{} reachable from the selected revisions:",
            if count == 1 { "" } else { "s" }
        );
        for (reference, tip) in &report.selected {
            message.push_str(&format!(
                "
- {reference} at {tip}"
            ));
        }
        message
    };
    if report.semantic_disabled {
        message.push_str(
            " Semantic indexing was disabled repository-wide; all cached semantic vectors were removed.",
        );
    }
    message
}

fn format_json_report(
    report: &QueryReport,
    additional_warnings: &[String],
    github_links: Option<&crate::github::LinksReport>,
) -> Result<String, serde_json::Error> {
    match report {
        QueryReport::PatchSearch(report) => {
            patch_search::format_json_report(report, additional_warnings)
        }
        QueryReport::FragmentSearch(report) => {
            fragment_search::format_json_report(report, additional_warnings, github_links)
        }
        QueryReport::Conflicts(report) => {
            conflicts::format_json_report(report, additional_warnings)
        }
        QueryReport::Followups(report) => {
            followups::format_json_report(report, additional_warnings)
        }
        QueryReport::Patterns(report) => patterns::format_json_report(report, additional_warnings),
        QueryReport::Hotspots(report) => hotspots::format_json_report(report, additional_warnings),
        QueryReport::Propagation(report) => {
            propagation::format_json_report(report, additional_warnings)
        }
        QueryReport::Context(report) => context::format_json_report(report, additional_warnings),
        QueryReport::TraceRemoval(report) => {
            trace_removal::format_json_report(report, additional_warnings)
        }
        QueryReport::TraceRemovalFragment(report) => {
            trace_removal::format_fragment_json_report(report, additional_warnings)
        }
        QueryReport::Analysis(report) => {
            json::format_json_report(report, additional_warnings, github_links)
        }
        QueryReport::Timeline(report) => {
            timeline::format_json_report(report, additional_warnings, github_links)
        }
        QueryReport::Fate(report) => {
            fate::format_json_report(report, additional_warnings, github_links)
        }
    }
}

/// Live query maintenance progress never enters machine-readable stdout.
pub(crate) fn refresh_progress(stage: IndexStage) {
    let label = match stage {
        IndexStage::ReadingCommits => "Refreshing pinned history: reading commits...",
        IndexStage::ReadingChanges => "Refreshing pinned history: reading changes...",
        IndexStage::ReadingPatches => "Refreshing pinned history: reading patches...",
        IndexStage::WritingCache => "Refreshing pinned history: writing cache...",
        IndexStage::Complete | IndexStage::BuildingSemanticIndex => return,
    };
    let _ = writeln!(io::stderr(), "{label}");
}

pub(crate) fn waiting_for_cache(message: &str) {
    let _ = writeln!(io::stderr(), "{message}");
}

pub(crate) fn run(command: Command) -> i32 {
    let json_output = command.uses_json();
    let index_scope = command.index_scope();
    let stderr = io::stderr();
    let is_terminal = stderr.is_terminal();
    let mut progress = IndexProgress::new(stderr.lock(), is_terminal, index_scope);
    let result = app::execute(command, &mut |stage| progress.report(stage));
    match progress.finish() {
        Ok(()) => finish_with_format(result, json_output),
        Err(error) => output_error(error),
    }
}

struct IndexProgress<W> {
    output: W,
    is_terminal: bool,
    /// Describes explicitly selected revisions; empty for current-HEAD indexing.
    index_scope: String,
    active: bool,
    error: Option<io::Error>,
}

impl<W: Write> IndexProgress<W> {
    fn new(output: W, is_terminal: bool, index_scope: String) -> Self {
        Self {
            output,
            is_terminal,
            index_scope,
            active: false,
            error: None,
        }
    }

    fn report(&mut self, progress: Progress) {
        match progress {
            Progress::Index(stage) => self.report_index(stage),
            Progress::Update(stage) => self.report_update(stage),
        }
    }

    fn report_index(&mut self, stage: IndexStage) {
        if self.error.is_some() {
            return;
        }
        let result = if self.is_terminal {
            self.active = !matches!(stage, IndexStage::Complete);
            let (filled, label) = match stage {
                IndexStage::ReadingCommits => (4, "Reading commits"),
                IndexStage::ReadingChanges => (8, "Reading changes"),
                IndexStage::ReadingPatches => (12, "Reading patches"),
                IndexStage::WritingCache => (16, "Writing cache"),
                IndexStage::BuildingSemanticIndex => (18, "Building semantic index"),
                IndexStage::Complete => (20, "Complete"),
            };
            let line = format!(
                "\r[{}{}] {label:<15}",
                "█".repeat(filled),
                " ".repeat(20 - filled),
            );
            if matches!(stage, IndexStage::Complete) {
                writeln!(self.output, "{line}")
            } else {
                write!(self.output, "{line}").and_then(|()| self.output.flush())
            }
        } else if matches!(stage, IndexStage::ReadingCommits) {
            if self.index_scope.is_empty() {
                writeln!(
                    self.output,
                    "Indexing history reachable from current HEAD..."
                )
            } else {
                writeln!(self.output, "{}", self.index_scope)
            }
        } else if matches!(stage, IndexStage::BuildingSemanticIndex) {
            writeln!(self.output, "Building semantic index...")
        } else {
            Ok(())
        };
        if let Err(error) = result {
            self.error = Some(error);
        }
    }

    fn report_update(&mut self, stage: UpdateStage) {
        if self.error.is_some() {
            return;
        }
        self.active = true;
        let label = match stage {
            UpdateStage::Checking => "Checking for updates...",
            UpdateStage::Downloading => "Downloading update...",
            UpdateStage::Verifying => "Verifying update...",
            UpdateStage::Installing => "Installing update...",
        };
        let result = if self.is_terminal {
            write!(self.output, "\r{label:<28}").and_then(|()| self.output.flush())
        } else {
            writeln!(self.output, "{label}")
        };
        if let Err(error) = result {
            self.error = Some(error);
        }
    }

    fn finish(mut self) -> io::Result<()> {
        if self.active && self.error.is_none() {
            writeln!(self.output)?;
        }
        match self.error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

fn output_error(error: io::Error) -> i32 {
    let _ = writeln!(io::stderr().lock(), "error: writing output: {error}");
    1
}
pub(crate) fn finish(result: Result<Outcome, AppError>) -> i32 {
    finish_with_format(result, false)
}

fn finish_with_format(result: Result<Outcome, AppError>, json_output: bool) -> i32 {
    match result {
        Ok(outcome) => match write_outcome(outcome, json_output) {
            Ok(()) => 0,
            Err(error) => output_error(error),
        },
        Err(error) => {
            let code = error.exit_code();
            let _ = write!(io::stderr().lock(), "{error}");
            if !error.to_string().ends_with('\n') {
                let _ = writeln!(io::stderr().lock());
            }
            code
        }
    }
}

fn write_outcome(outcome: Outcome, json_output: bool) -> io::Result<()> {
    let mut stderr = io::stderr().lock();
    for progress in outcome
        .progress
        .iter()
        .filter(|line| !line.starts_with("Waiting for another GitScry process"))
    {
        writeln!(stderr, "{progress}")?;
    }

    if let Some(report) = &outcome.usage_report {
        if json_output {
            let json = usage::format_json(report)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            return writeln!(io::stdout().lock(), "{json}");
        }
        return writeln!(io::stdout().lock(), "{}", usage::format_text(report));
    }

    if let Some(report) = &outcome.clear_report {
        return writeln!(io::stdout().lock(), "{}", clear::format_report(report));
    }
    if let Some(report) = &outcome.index_report {
        return writeln!(io::stdout().lock(), "{}", format_index_report(report));
    }
    if let Some(report) = &outcome.prune_report {
        return writeln!(io::stdout().lock(), "{}", prune::format_report(report));
    }
    if json_output {
        let report = outcome.report.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "JSON output requires a query report",
            )
        })?;
        let json = format_json_report(report, &outcome.warnings, outcome.github_links.as_ref())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        write_query_output(&format!("{json}\n"), &mut stderr)
    } else {
        for warning in &outcome.warnings {
            writeln!(stderr, "{warning}")?;
        }
        if let Some(report) = &outcome.report {
            for warning in report.human_warnings().iter() {
                writeln!(stderr, "{warning}")?;
            }
        }
        for notice in &outcome.notices {
            writeln!(stderr, "{notice}")?;
        }
        let message = outcome.report.as_ref().map_or_else(
            || outcome.message.clone(),
            |report| format_report(report, outcome.github_links.as_ref()),
        );
        if outcome.report.is_some() {
            write_query_output(&format!("{message}\n"), &mut stderr)
        } else {
            writeln!(io::stdout().lock(), "{message}")
        }
    }
}

fn write_query_output(original: &str, stderr: &mut impl Write) -> io::Result<()> {
    output::write_query(
        original,
        &mut io::stdout().lock(),
        stderr,
        std::env::var_os("GITSCRY_FULL_OUTPUT").is_some_and(|value| value == "1"),
        &std::env::temp_dir(),
    )
}

pub(crate) fn display(text: &str) -> i32 {
    match write!(io::stdout().lock(), "{text}") {
        Ok(()) => 0,
        Err(error) => output_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_progress_reaches_complete() {
        let mut output = Vec::new();
        let mut progress = IndexProgress::new(&mut output, true, String::new());
        for stage in [
            IndexStage::ReadingCommits,
            IndexStage::ReadingChanges,
            IndexStage::ReadingPatches,
            IndexStage::WritingCache,
            IndexStage::BuildingSemanticIndex,
        ] {
            let previous_len = progress.output.len();
            progress.report(Progress::Index(stage));
            let frame = &progress.output[previous_len..];
            assert!(frame.starts_with(b"\r"));
            assert!(!frame.contains(&b'\n'));
            assert!(
                String::from_utf8_lossy(frame)
                    .chars()
                    .any(char::is_alphabetic)
            );
        }
        let previous_len = progress.output.len();
        progress.report(Progress::Index(IndexStage::Complete));
        let completion = &progress.output[previous_len..];
        assert!(completion.starts_with(b"\r"));
        assert!(completion.ends_with(b"\n"));
        assert_eq!(completion.iter().filter(|byte| **byte == b'\n').count(), 1);
        assert!(
            String::from_utf8_lossy(completion)
                .chars()
                .any(char::is_alphabetic)
        );
        let completed_len = progress.output.len();
        progress.finish().unwrap();

        assert_eq!(output.len(), completed_len);
    }

    #[test]
    fn redirected_progress_preserves_plain_text_contract() {
        let mut output = Vec::new();
        let mut progress = IndexProgress::new(&mut output, false, String::new());
        for stage in [
            IndexStage::ReadingCommits,
            IndexStage::ReadingChanges,
            IndexStage::ReadingPatches,
            IndexStage::WritingCache,
            IndexStage::Complete,
        ] {
            progress.report(Progress::Index(stage));
        }
        progress.finish().unwrap();

        let output = String::from_utf8(output).unwrap();
        assert_eq!(output.lines().count(), 1);
        assert!(!output.trim().is_empty());
        assert!(output.ends_with('\n'));
        assert!(!output.contains('\r'));
        assert!(!output.contains('\x1b'));
    }
}
