use std::io::{self, Write};

use crate::{
    analysis::SearchReport,
    app::{AppError, Outcome},
};

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

pub(crate) fn format_search(report: &SearchReport) -> String {
    if report.materials.is_empty() {
        return "No relevant history found.".to_owned();
    }

    let mut lines = vec![format!(
        "Relevant history ({} match{}):",
        report.matched_count,
        if report.matched_count == 1 { "" } else { "es" },
    )];
    for material in &report.materials {
        let subject = if material.subject.is_empty() {
            "(no subject)"
        } else {
            material.subject.trim()
        };
        lines.push(format!(
            "- {} {}",
            material.abbreviation,
            render_subject(subject)
        ));
        for path in material.paths.iter().take(5) {
            lines.push(format!("  path: {}", render_path(path)));
        }
        if material.paths.len() > 5 {
            lines.push(format!(
                "  ... {} more path{}",
                material.paths.len() - 5,
                if material.paths.len() - 5 == 1 {
                    ""
                } else {
                    "s"
                },
            ));
        }
        lines.push(format!("  confidence: {}", material.confidence.as_str()));
        lines.push(format!("  basis: {}", material.basis.join(", ")));
    }
    if report.truncated {
        lines.push(format!(
            "Showing {} of {} matching commits; results truncated.",
            report.materials.len(),
            report.matched_count,
        ));
    }
    lines.join("\n")
}

fn render_subject(subject: &str) -> String {
    let mut rendered = String::with_capacity(subject.len());
    for character in subject.chars() {
        if character.is_control() {
            rendered.extend(character.escape_default());
        } else {
            rendered.push(character);
        }
    }
    rendered
}

fn render_path(path: &[u8]) -> String {
    let quoted = path
        .iter()
        .any(|byte| !(0x21..=0x7e).contains(byte) || *byte == b'"' || *byte == b'\\');
    let mut rendered = String::with_capacity(path.len() + usize::from(quoted) * 2);
    if quoted {
        rendered.push('"');
    }
    for byte in path {
        match byte {
            b'\\' => rendered.push_str("\\\\"),
            b'"' => rendered.push_str("\\\""),
            b'\n' => rendered.push_str("\\n"),
            b'\r' => rendered.push_str("\\r"),
            b'\t' => rendered.push_str("\\t"),
            0x21..=0x7e => rendered.push(*byte as char),
            _ => rendered.push_str(&format!("\\{byte:03o}")),
        }
    }
    if quoted {
        rendered.push('"');
    }
    rendered
}
