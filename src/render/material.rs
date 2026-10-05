use crate::analysis::{
    Detail, Failure, Material, PatchExcerpt, PatchStatus, Relation, Report, ReportKind,
    SearchScopeInfo, Step, SymbolFact, SymbolSummary, WhyAttribution,
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum GenericReportKind {
    Search,
    CodeSearch,
    Examples,
    Failures,
    Related,
    Tests,
    Regression,
    TraceFix,
}

impl GenericReportKind {
    fn from_report_kind(kind: ReportKind) -> Option<Self> {
        match kind {
            ReportKind::Why => None,
            ReportKind::Search => Some(Self::Search),
            ReportKind::CodeSearch => Some(Self::CodeSearch),
            ReportKind::Examples => Some(Self::Examples),
            ReportKind::Failures => Some(Self::Failures),
            ReportKind::Related => Some(Self::Related),
            ReportKind::Tests => Some(Self::Tests),
            ReportKind::Regression => Some(Self::Regression),
            ReportKind::TraceFix => Some(Self::TraceFix),
        }
    }
}
/// Fixed no-result text, so absence of history is distinguished from a failure.
fn empty_message(kind: GenericReportKind) -> &'static str {
    match kind {
        GenericReportKind::Search => "No relevant history found.",
        GenericReportKind::CodeSearch => "No matching changed lines found.",
        GenericReportKind::Examples => "No historical examples found.",
        GenericReportKind::Failures => "No failed approaches found.",
        GenericReportKind::Related => "No historical relations found.",
        GenericReportKind::Tests => "No historically related tests found.",
        GenericReportKind::Regression => "No supported regression suspects found.",
        GenericReportKind::TraceFix => "No introducing change could be traced.",
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
    let coverage = if scope.coverage_complete {
        "complete".to_owned()
    } else {
        "incomplete; only cached reachable history is included".to_owned()
    };
    format!(
        "Scope: {}; cache tip {}; coverage {coverage}",
        criteria.join("; "),
        scope.cache_tip,
    )
}
fn format_why_report(report: &Report) -> String {
    let Some(why) = report.why.as_ref() else {
        return "Why summary unavailable.".to_owned();
    };
    let mut lines = Vec::new();
    if let Some(scope) = &report.scope {
        lines.push(scope_summary(scope));
    }
    if let Some(summary) = &report.symbol_summary {
        lines.extend(format_symbol_summary(summary));
    }
    lines.push(format!(
        "Why at {} at revision {}:",
        escape::subject(&why.anchor),
        escape::subject(&why.revision)
    ));
    let attribution_scope = match why.attribution_scope {
        "target_line" => "target line",
        "symbol_starting_line_only" => "symbol starting line only",
        _ => why.attribution_scope,
    };
    lines.push(format!("Attribution scope: {attribution_scope}."));
    let attribution_count = usize::from(matches!(&why.attribution, WhyAttribution::Available(_)));
    lines.push(format!("Attribution count: {attribution_count}"));
    match &why.attribution {
        WhyAttribution::Available(attribution) => {
            lines.push(format!(
                "Attribution: {} {}",
                short_oid(&attribution.oid),
                escape::subject(&attribution.subject)
            ));
            lines.push(format!("  basis: {}", attribution.basis.join(", ")));
            if attribution.shallow_boundary {
                lines.push("  attribution reaches a shallow-history boundary.".to_owned());
            }
            if attribution.consolidated_target_modification {
                lines.push(
                    "  this commit is also a target-related modification and is shown once here."
                        .to_owned(),
                );
            }
            render_patch(&mut lines, attribution.patch.as_ref(), "  ");
        }
        WhyAttribution::OutsideHistoricalScope => lines.push(
            "Attribution: outside the requested historical scope; no out-of-scope commit is shown."
                .to_owned(),
        ),
        WhyAttribution::Unavailable { reason } => lines.push(format!(
            "Attribution unavailable: {}",
            escape::subject(reason)
        )),
    }

    lines.push(format!(
        "Standalone target-related modifications ({}):",
        why.standalone_target_related_modification_count
    ));
    if why.target_related_modifications.is_empty() {
        lines
            .push("  No standalone target-related modifications in the selected scope.".to_owned());
    }
    for modification in &why.target_related_modifications {
        lines.push(format!(
            "- {} {}",
            short_oid(&modification.oid),
            escape::subject(&modification.subject)
        ));
        if !modification.paths.is_empty() {
            lines.push(format!(
                "  paths: {}",
                modification
                    .paths
                    .iter()
                    .map(|path| escape::path(path))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        lines.push(format!("  basis: {}", modification.basis.join(", ")));
        render_patch(&mut lines, modification.patch.as_ref(), "  ");
    }
    lines.push(format!(
        "Other file history: {}",
        why.other_file_history_count
    ));
    lines.push(format!("File history in scope: {}", why.file_history_count));
    if why.omitted_target_related_modifications > 0 {
        lines.push(format!(
            "Omitted {} target-related modification{} due to --limit.",
            why.omitted_target_related_modifications,
            if why.omitted_target_related_modifications == 1 {
                ""
            } else {
                "s"
            }
        ));
    }
    if let Some(args) = &why.timeline_follow_up_args {
        let command = args
            .iter()
            .map(|argument| shell_argument(argument))
            .collect::<Vec<_>>()
            .join(" ");
        lines.push(format!("Timeline follow-up: `gitscry {command}`"));
    }
    lines.push("Limitations:".to_owned());
    lines.extend(
        why.limitations
            .iter()
            .map(|limitation| format!("- {}", escape::subject(limitation))),
    );
    lines.join("\n")
}

fn format_symbol_summary(summary: &SymbolSummary) -> Vec<String> {
    let mut lines = vec!["Symbol summary:".to_owned()];
    lines.push(format!("target: {}", escape::subject(&summary.target)));
    for (label, fact) in [
        ("introduction", &summary.introduction),
        ("anchor-line attribution", &summary.anchor_line_attribution),
    ] {
        let line = match fact {
            SymbolFact::Known {
                commit_oid,
                subject,
            } => format!(
                "  {label}: known {} — {}",
                short_oid(commit_oid),
                escape::subject(subject.trim()),
            ),
            SymbolFact::Unknown { reason } => {
                format!("  {label}: unknown — {}", escape::subject(reason.trim()))
            }
        };
        lines.push(line);
    }
    lines
}

fn short_oid(oid: &str) -> String {
    oid.chars().take(12).collect()
}

fn shell_argument(argument: &str) -> String {
    if argument
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"_./:-".contains(&byte))
    {
        argument.to_owned()
    } else {
        format!("'{}'", argument.replace('\'', "'\\''"))
    }
}

fn format_code_report(report: &Report) -> String {
    let mut lines = Vec::new();
    if let Some(scope) = &report.scope {
        lines.push(scope_summary(scope));
    }
    if report.code_matches.is_empty() {
        lines.push(empty_message(GenericReportKind::CodeSearch).to_owned());
        return lines.join("\n");
    }
    lines.push(header(report, GenericReportKind::CodeSearch));
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
    let Some(kind) = GenericReportKind::from_report_kind(report.kind) else {
        return format_why_report(report);
    };
    if kind == GenericReportKind::CodeSearch {
        return format_code_report(report);
    }
    let mut lines = if report.materials.is_empty() {
        vec![empty_message(kind).to_owned()]
    } else {
        let mut lines = vec![header(report, kind)];
        for material in &report.materials {
            render_material(&mut lines, material);
        }
        if report.truncated {
            let noun = match kind {
                GenericReportKind::CodeSearch => "matching lines",
                GenericReportKind::Related | GenericReportKind::Tests => "matching paths",
                GenericReportKind::Search
                | GenericReportKind::Examples
                | GenericReportKind::Failures
                | GenericReportKind::TraceFix => "matching commits",
                GenericReportKind::Regression => "matching suspects",
            };
            lines.push(format!(
                "Showing {} of {} {noun}; results truncated.",
                report.materials.len(),
                report.matched_count,
            ));
        }
        lines
    };
    if let Some(summary) = &report.symbol_summary {
        lines.splice(0..0, format_symbol_summary(summary));
    }
    if let Some(scope) = &report.scope {
        lines.insert(0, scope_summary(scope));
    }
    lines.join("\n")
}

fn header(report: &Report, kind: GenericReportKind) -> String {
    let noun = match kind {
        GenericReportKind::Search => "Relevant history",
        GenericReportKind::CodeSearch => "Code search",
        GenericReportKind::Examples => "Historical examples",
        GenericReportKind::Failures => "Failed approaches",
        GenericReportKind::Related => "Related paths",
        GenericReportKind::Tests => "Historical test candidates",
        GenericReportKind::Regression => "Regression suspects",
        GenericReportKind::TraceFix => "Fix lineage",
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
        .filter(|citation| relation.co_change_citations.contains(&citation.oid))
        .take(RELATION_CITATIONS_PER_RESULT)
    {
        let subject = escape::subject(citation.subject.trim());
        lines.push(format!(
            "  supporting commit: {} {}",
            citation.abbreviation, subject,
        ));
    }
    let shown_citations = relation
        .co_change_citations
        .len()
        .min(RELATION_CITATIONS_PER_RESULT);
    let remaining = relation.supporting_count.saturating_sub(shown_citations);
    if remaining > 0 {
        lines.push(format!(
            "  ... {remaining} more supporting commit{}",
            if remaining == 1 { "" } else { "s" }
        ));
    }
    for evidence in &relation.follow_on {
        render_follow_on(lines, evidence);
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

pub(super) fn render_follow_on(
    lines: &mut Vec<String>,
    evidence: &crate::analysis::PathObservation,
) {
    let observation = &evidence.observation;
    lines.push(format!(
        "  follow-on: {} -> {}",
        escape::path(&evidence.source),
        escape::path(&observation.path)
    ));
    lines.push(format!(
        "    supporting origins: {}/{} eligible complete origins; {} independent chains",
        observation.supporting_origins,
        observation.eligible_origins,
        observation.independent_chains
    ));
    lines.push(format!(
        "    background: {}/{} uniformly sampled complete origins (at most 100)",
        observation.baseline_occurrences, observation.baseline_sample_size
    ));
    lines.push(format!("    window: {} days and {} shortest parent edges, inclusive; proper descendants; non-merge events", observation.observation_days, crate::analysis::FOLLOW_ON_MAX_PARENT_DISTANCE));
    lines.push("    denominator: candidate-touching origins excluded from source and background pools; incomplete/unobserved windows excluded".to_owned());
    lines.push("    identity: detected renames only; copies and recreation start new incarnations; historical association, not causation".to_owned());
    for chain in &observation.chains {
        lines.push(format!(
            "    example: {} {} -> {} {}; {} parent edges, {} seconds",
            chain.origin_oid,
            escape::path(&chain.origin_path),
            chain.later_oid,
            escape::path(&chain.later_path),
            chain.parent_distance,
            chain.elapsed_seconds
        ));
    }
    lines.push(format!(
        "    omitted examples: {}",
        observation.omitted_examples
    ));
}
