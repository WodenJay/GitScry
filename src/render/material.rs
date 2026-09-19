use crate::analysis::{Detail, Failure, Material, Relation, Report, ReportKind, Step};

/// Printed when history does not state why an approach failed.
const REASON_UNKNOWN: &str = "Reason unknown";

use super::escape;

/// Paths shown per result before the remainder is summarised.
const PATHS_PER_RESULT: usize = 5;
/// Steps shown per result; more would bury the reusable pattern in a file listing.
const STEPS_PER_RESULT: usize = 8;

/// Fixed no-result text, so absence of history is distinguished from a failure.
fn empty_message(kind: ReportKind) -> &'static str {
    match kind {
        ReportKind::Search => "No relevant history found.",
        ReportKind::Examples => "No historical examples found.",
        ReportKind::Failures => "No failed approaches found.",
        ReportKind::Related => "No historical relations found.",
        ReportKind::Tests => "No historically related tests found.",
    }
}

pub(crate) fn format_report(report: &Report) -> String {
    if report.materials.is_empty() {
        return empty_message(report.kind).to_owned();
    }

    let mut lines = vec![header(report)];
    for material in &report.materials {
        render_material(&mut lines, material);
    }
    if report.truncated {
        let noun = match report.kind {
            ReportKind::Related | ReportKind::Tests => "matching paths",
            ReportKind::Search | ReportKind::Examples | ReportKind::Failures => "matching commits",
        };
        lines.push(format!(
            "Showing {} of {} {noun}; results truncated.",
            report.materials.len(),
            report.matched_count,
        ));
    }
    lines.join("\n")
}

fn header(report: &Report) -> String {
    let noun = match report.kind {
        ReportKind::Search => "Relevant history",
        ReportKind::Examples => "Historical examples",
        ReportKind::Failures => "Failed approaches",
        ReportKind::Related => "Related paths",
        ReportKind::Tests => "Historical test candidates",
    };
    format!(
        "{noun} ({} match{}):",
        report.matched_count,
        if report.matched_count == 1 { "" } else { "es" },
    )
}

fn render_material(lines: &mut Vec<String>, material: &Material) {
    if let Some(Detail::Relation(relation)) = material.detail.as_ref() {
        render_relation(lines, material, relation);
        return;
    }
    let subject = if material.subject.is_empty() {
        "(no subject)"
    } else {
        material.subject.trim()
    };
    match material.citations.first() {
        Some(primary) => lines.push(format!(
            "- {} {}",
            primary.abbreviation,
            escape::subject(subject)
        )),
        None => lines.push(format!("- {}", escape::subject(subject))),
    }

    render_detail(lines, &material.detail);
    render_paths(lines, &material.paths);
    lines.push(format!("  confidence: {}", material.confidence.as_str()));
    lines.push(format!("  basis: {}", material.basis.join(", ")));
    render_related_commits(lines, material);
}

fn render_relation(lines: &mut Vec<String>, material: &Material, relation: &Relation) {
    let path = material
        .paths
        .first()
        .map(|path| escape::path(path))
        .unwrap_or_default();
    lines.push(format!("- candidate path: {path}"));
    lines.push(format!("  co-change count: {}", relation.co_change_count));
    lines.push(format!("  proportion: {:.1}%", relation.proportion * 100.0));
    lines.push(format!(
        "  supporting commits: {}",
        relation.supporting_count
    ));
    for citation in &material.citations {
        let subject = escape::subject(citation.subject.trim());
        lines.push(format!(
            "  supporting commit: {} {}",
            citation.abbreviation, subject,
        ));
    }
    if material.citations.len() < relation.supporting_count {
        let remaining = relation.supporting_count - material.citations.len();
        lines.push(format!(
            "  ... {remaining} more supporting commit{}",
            if remaining == 1 { "" } else { "s" }
        ));
    }
    lines.push(format!("  confidence: {}", material.confidence.as_str()));
    lines.push(format!("  basis: {}", material.basis.join(", ")));
}

fn render_detail(lines: &mut Vec<String>, detail: &Option<Detail>) {
    match detail {
        // Steps read as what history did, never as what the caller must do.
        Some(Detail::Steps(steps)) => {
            for step in steps.iter().take(STEPS_PER_RESULT) {
                lines.push(format!("  step: {}", describe_step(step)));
            }
            if steps.len() > STEPS_PER_RESULT {
                let remaining = steps.len() - STEPS_PER_RESULT;
                lines.push(format!(
                    "  ... {remaining} more step{}",
                    if remaining == 1 { "" } else { "s" }
                ));
            }
        }
        Some(Detail::Failure(failure)) => {
            lines.push(format!("  reason: {}", describe(failure)));
            if let Some(retry) = failure.retry.as_deref() {
                lines.push(format!("  retry: {retry}"));
            }
        }
        Some(Detail::Relation(_)) => {}
        None => {}
    }
}

/// A precedent, phrased as a historical move rather than an instruction.
fn describe_step(step: &Step) -> String {
    match step {
        Step::Removed(path) => format!("remove {}", escape::path(path)),
        Step::Moved(old, new) => format!("move {} to {}", escape::path(old), escape::path(new)),
        Step::Added(path) => format!("add {}", escape::path(path)),
        Step::Modified(path) => format!("update {}", escape::path(path)),
    }
}

fn describe(failure: &Failure) -> String {
    failure
        .reason
        .clone()
        .unwrap_or_else(|| REASON_UNKNOWN.to_owned())
}

fn render_paths(lines: &mut Vec<String>, paths: &[Vec<u8>]) {
    for path in paths.iter().take(PATHS_PER_RESULT) {
        lines.push(format!("  path: {}", escape::path(path)));
    }
    if paths.len() > PATHS_PER_RESULT {
        let remaining = paths.len() - PATHS_PER_RESULT;
        lines.push(format!(
            "  ... {remaining} more path{}",
            if remaining == 1 { "" } else { "s" }
        ));
    }
}

/// Supporting commits, so every claim in the material is traceable to Git.
fn render_related_commits(lines: &mut Vec<String>, material: &Material) {
    let supporting = material.citations.iter().skip(1);
    for citation in supporting {
        let subject = escape::subject(citation.subject.trim());
        match citation.note {
            Some(note) => lines.push(format!(
                "  commit: {} {} ({note})",
                citation.abbreviation, subject
            )),
            None => lines.push(format!("  commit: {} {}", citation.abbreviation, subject)),
        }
    }
}
