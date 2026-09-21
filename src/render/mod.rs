//! The only place GitScry writes to stdout and stderr.
//!
//! One reason to change: the user-visible text contract. Progress and warnings go to
//! stderr, material and no-result messages to stdout, and every result is human-readable
//! English rather than a stable machine format.

mod escape;
mod material;

use crate::{
    app::{self, AppError, IndexStage, Outcome},
    cli::Command,
};
use std::io::{self, IsTerminal, Write};

pub(crate) use material::format_report;

pub(crate) fn run(command: Command) -> i32 {
    let stderr = io::stderr();
    let is_terminal = stderr.is_terminal();
    let mut progress = IndexProgress::new(stderr.lock(), is_terminal);
    let result = app::execute(command, &mut |stage| progress.report(stage));
    match progress.finish() {
        Ok(()) => finish(result),
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

    fn report(&mut self, stage: IndexStage) {
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
        } else {
            Ok(())
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
    match result {
        Ok(outcome) => match write_outcome(outcome) {
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

fn write_outcome(outcome: Outcome) -> io::Result<()> {
    let mut stderr = io::stderr().lock();
    for progress in outcome.progress {
        writeln!(stderr, "{progress}")?;
    }
    for notice in outcome.notices {
        writeln!(stderr, "{notice}")?;
    }
    writeln!(io::stdout().lock(), "{}", outcome.message)
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
            progress.report(stage);
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
            progress.report(stage);
        }
        progress.finish().unwrap();

        assert_eq!(output, b"Indexing local history...\n");
    }
}
