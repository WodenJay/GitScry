//! The only place GitScry writes to stdout and stderr.
//!
//! One reason to change: user-visible output formats. Process progress and warnings stay on
//! stderr; query reports go to stdout as either human-readable English or schema-versioned JSON.

mod context;
mod escape;
mod followups;
mod github_links;
mod json;
mod material;

mod patterns;
mod timeline;
mod trace_removal;
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
        QueryReport::Followups(report) => followups::format_report(report),
        QueryReport::Patterns(report) => patterns::format_report(report),
        QueryReport::Context(report) => context::format_report(report),
        QueryReport::TraceRemoval(report) => trace_removal::format_report(report),
        QueryReport::Analysis(report) => {
            let mut output = material::format_report(report);
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
    }
}

fn format_json_report(
    report: &QueryReport,
    additional_warnings: &[String],
    github_links: Option<&crate::github::LinksReport>,
) -> Result<String, serde_json::Error> {
    match report {
        QueryReport::Followups(report) => {
            followups::format_json_report(report, additional_warnings)
        }
        QueryReport::Patterns(report) => patterns::format_json_report(report, additional_warnings),
        QueryReport::Context(report) => context::format_json_report(report, additional_warnings),
        QueryReport::TraceRemoval(report) => {
            trace_removal::format_json_report(report, additional_warnings)
        }
        QueryReport::Analysis(report) => {
            json::format_json_report(report, additional_warnings, github_links)
        }
        QueryReport::Timeline(report) => {
            timeline::format_json_report(report, additional_warnings, github_links)
        }
    }
}

pub(crate) fn run(command: Command) -> i32 {
    let json_output = command.uses_json();
    let stderr = io::stderr();
    let is_terminal = stderr.is_terminal();
    let mut progress = IndexProgress::new(stderr.lock(), is_terminal);
    let result = app::execute(command, &mut |stage| progress.report(stage));
    match progress.finish() {
        Ok(()) => finish_with_format(result, json_output),
        Err(error) => output_error(error),
    }
}

struct IndexProgress<W> {
    output: W,
    is_terminal: bool,
    active: bool,
    error: Option<io::Error>,
}

impl<W: Write> IndexProgress<W> {
    fn new(output: W, is_terminal: bool) -> Self {
        Self {
            output,
            is_terminal,
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
            writeln!(self.output, "Indexing local history...")
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
    for progress in &outcome.progress {
        writeln!(stderr, "{progress}")?;
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
        writeln!(io::stdout().lock(), "{json}")
    } else {
        for warning in &outcome.warnings {
            writeln!(stderr, "{warning}")?;
        }
        if let Some(report) = &outcome.report {
            for warning in report.warnings() {
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
        writeln!(io::stdout().lock(), "{message}")
    }
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
        let mut progress = IndexProgress::new(&mut output, true);
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
        assert_eq!(output.matches('\r').count(), 5);
        assert!(output.contains("[████                ] Reading commits"));
        assert!(output.ends_with("[████████████████████] Complete       \n"));
    }

    #[test]
    fn redirected_progress_preserves_plain_text_contract() {
        let mut output = Vec::new();
        let mut progress = IndexProgress::new(&mut output, false);
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

        assert_eq!(output, b"Indexing local history...\n");
    }
}
