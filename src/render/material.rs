use crate::analysis::{
    Detail, Failure, Material, PatchExcerpt, PatchStatus, Relation, Report, ReportKind,
    SearchScopeInfo, Step,
};

/// Printed when history does not state why an approach failed.
const REASON_UNKNOWN: &str = "Reason unknown";

use super::escape;

/// Paths shown per result before the remainder is summarised.
const PATHS_PER_RESULT: usize = 5;
/// Steps shown per result; more would bury the reusable pattern in a file listing.
const STEPS_PER_RESULT: usize = 8;

/// Supporting commits shown per related path before the remainder is summarized.
const RELATION_CITATIONS_PER_RESULT: usize = 5;
/// Fixed no-result text, so absence of history is distinguished from a failure.
fn empty_message(kind: ReportKind) -> &'static str {
    match kind {
        ReportKind::Search => "No relevant history found.",
        ReportKind::CodeSearch => "No matching changed lines found.",
        ReportKind::Examples => "No historical examples found.",
        ReportKind::Failures => "No failed approaches found.",
        ReportKind::Related => "No historical relations found.",
        ReportKind::Tests => "No historically related tests found.",
        ReportKind::Why => "No explanatory history found.",
        ReportKind::Regression => "No supported regression suspects found.",
        ReportKind::TraceFix => "No introducing change could be traced.",
    }
}

pub(super) fn scope_summary(scope: &SearchScopeInfo) -> String {
    let mut criteria = vec![format!("commits reachable from {}", scope.to_rev)];
    if let Some(target_rev) = &scope.target_rev {
        criteria.push(format!("intersected with target revision {target_rev}"));
    }
    if let Some(from_rev) = &scope.from_rev {
        criteria.push(format!("excluding {from_rev} and its ancestors"));
    }
    if let Some(since) = &scope.since {
        criteria.push(format!("committer time since {since}"));
    }
    if let Some(until) = &scope.until {
        criteria.push(format!("committer time through {until}"));
    }
    format!(
        "Scope: {}; cache tip {}",
        criteria.join("; "),
        scope.cache_tip,
    )
}

fn format_code_report(report: &Report) -> String {
    let mut lines = Vec::new();
    if let Some(scope) = &report.scope {
        lines.push(scope_summary(scope));
    }
    if report.code_matches.is_empty() {
        lines.push(empty_message(report.kind).to_owned());
        return lines.join("\n");
    }
    lines.push(header(report));
    for matched in &report.code_matches {
        lines.push(format!(
            "- {} {}:{} {}: {}",
            matched.commit_id,
            escape::path(&matched.path),
            matched.line_number,
            matched.direction.as_str(),
            escape::code_line(&matched.line),
        ));
    }
    if report.truncated {
        lines.push(format!(
            "Showing {} of {} matching lines; results truncated.",
            report.code_matches.len(),
            report.matched_count,
        ));
    }
    lines.join("\n")
}

pub(crate) fn format_report(report: &Report) -> String {
    if report.kind == ReportKind::CodeSearch {
        return format_code_report(report);
    }
    let mut lines = if report.materials.is_empty() {
        vec![empty_message(report.kind).to_owned()]
    } else {
        let mut lines = vec![header(report)];
        for material in &report.materials {
            render_material(&mut lines, material);
        }
        if report.truncated {
            let noun = match report.kind {
                ReportKind::CodeSearch => "matching lines",
                ReportKind::Related | ReportKind::Tests => "matching paths",
                ReportKind::Why => "matching commits",
                ReportKind::Search
                | ReportKind::Examples
                | ReportKind::Failures
                | ReportKind::TraceFix => "matching commits",
                ReportKind::Regression => "matching suspects",
            };
            lines.push(format!(
                "Showing {} of {} {noun}; results truncated.",
                report.materials.len(),
                report.matched_count,
            ));
        }
        lines
    };
    if let Some(scope) = &report.scope {
        lines.insert(0, scope_summary(scope));
    }
    lines.join("\n")
}

fn header(report: &Report) -> String {
    let noun = match report.kind {
        ReportKind::Search => "Relevant history",
        ReportKind::CodeSearch => "Code search",
        ReportKind::Examples => "Historical examples",
        ReportKind::Failures => "Failed approaches",
        ReportKind::Related => "Related paths",
        ReportKind::Tests => "Historical test candidates",
        ReportKind::Why => "Why history",
        ReportKind::Regression => "Regression suspects",
        ReportKind::TraceFix => "Fix lineage",
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
    render_patch(lines, material.patch.as_ref(), "  ");
}

pub(super) fn render_patch(lines: &mut Vec<String>, patch: Option<&PatchExcerpt>, indent: &str) {
    let Some(patch) = patch else {
        return;
    };
    let status = match patch.status {
        PatchStatus::Available => "available",
        PatchStatus::NoRelevantHunks | PatchStatus::NoRelevantHunk => {
            "No relevant text hunk found."
        }
        PatchStatus::Unavailable => "Text hunk unavailable.",
    };
    lines.push(format!(
        "{indent}patch excerpt: {status} (commit {})",
        patch.commit_oid
    ));
    for hunk in &patch.hunks {
        render_patch_hunk(lines, hunk, indent);
    }
    if patch.truncated {
        lines.push(format!("{indent}patch excerpt truncated by safety limits."));
    }
}

fn render_patch_hunk(lines: &mut Vec<String>, hunk: &crate::analysis::PatchHunk, indent: &str) {
    let old_path = hunk.old_path.as_deref().map(escape::path);
    let new_path = hunk.new_path.as_deref().map(escape::path);
    let path = match (old_path, new_path) {
        (Some(old), Some(new)) if old != new => format!("{old} -> {new}"),
        (Some(_), Some(new)) => new,
        (Some(old), None) => old,
        (None, Some(new)) => new,
        (None, None) => "(unknown path)".to_owned(),
    };
    lines.push(format!(
        "{indent}hunk: {path} (old {}+{}, new {}+{})",
        hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines
    ));
    let content_indent = format!("{indent}  ");
    if let Some(text) = &hunk.text {
        for line in text
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            lines.push(format!("{content_indent}{}", escape::code_line(line)));
        }
    } else {
        lines.push(format!(
            "{content_indent}[hunk text omitted: exceeds the scan limit]"
        ));
    }
    if hunk.truncated {
        lines.push(format!("{content_indent}[hunk excerpt truncated]"));
    }
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
    for citation in material
        .citations
        .iter()
        .take(RELATION_CITATIONS_PER_RESULT)
    {
        let subject = escape::subject(citation.subject.trim());
        lines.push(format!(
            "  supporting commit: {} {}",
            citation.abbreviation, subject,
        ));
    }
    let shown_citations = material.citations.len().min(RELATION_CITATIONS_PER_RESULT);
    let remaining = relation.supporting_count.saturating_sub(shown_citations);
    if remaining > 0 {
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
        Some(Detail::Why(why)) => {
            lines.push(format!(
                "  target: {} at {} (line {})",
                escape::subject(&why.anchor),
                escape::subject(&why.revision),
                why.line
            ));
        }
        Some(Detail::TraceFix(trace)) => {
            lines.push(format!(
                "  trace: {} at fix {}",
                trace.role,
                escape::subject(&trace.fix_revision),
            ));
            if let Some(parent) = &trace.parent_revision {
                lines.push(format!("  parent: {}", escape::subject(parent)));
            }
            if let Some(line) = trace.line {
                lines.push(format!("  deleted line: {line}"));
            }
        }
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
