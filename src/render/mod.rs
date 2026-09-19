//! The only place GitScry writes to stdout and stderr.
//!
//! One reason to change: the user-visible text contract. Progress and warnings go to
//! stderr, material and no-result messages to stdout, and every result is human-readable
//! English rather than a stable machine format.

mod escape;
mod material;

use std::io::{self, Write};

use crate::app::{AppError, Outcome};

pub(crate) use material::format_report;

pub(crate) fn finish(result: Result<Outcome, AppError>) -> i32 {
    match result {
        Ok(outcome) => match write_outcome(outcome) {
            Ok(()) => 0,
            Err(error) => {
                let _ = writeln!(io::stderr().lock(), "error: writing output: {error}");
                1
            }
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
    writeln!(io::stdout().lock(), "{}", outcome.message)
}

pub(crate) fn display(text: &str) -> i32 {
    match write!(io::stdout().lock(), "{text}") {
        Ok(()) => 0,
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "error: writing output: {error}");
            1
        }
    }
}
