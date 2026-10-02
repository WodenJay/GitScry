use super::{
    escape,
    json::{self, JsonPath, JsonSearchScope},
    material,
};
use crate::analysis::{ContextReport, ContextSuggestion};
use serde::Serialize;

pub(super) fn format_report(report: &ContextReport) -> String {
    let mode = if report.input.staged {
        "HEAD-to-index"
    } else {
        "HEAD-to-worktree + unignored untracked"
    };
    let mut lines = vec![format!("Current-change context ({mode})")];
    if let Some(head) = &report.input.head {
        lines.push(format!("Input HEAD: {head}"));
    }
    if report.input.changes.is_empty() {
        lines.push("No selected current change; cache not opened.".to_owned());
    }
    for change in &report.input.changes {
        let old = change
            .old_path
            .as_deref()
            .map(escape::path)
            .unwrap_or_else(|| "-".to_owned());
        let new = change
            .new_path
            .as_deref()
            .map(escape::path)
            .unwrap_or_else(|| "-".to_owned());
        lines.push(format!("Input {}: {old} -> {new}", change.status));
    }
    if let Some(tip) = &report.cache_tip {
        lines.push(format!("Published cache tip: {tip}"));
    }
    if let Some(scope) = &report.scope {
        lines.push(material::scope_summary(scope));
    } else if report.cache_tip.is_some() {
        lines.push("Historical scope: all available published cache history".to_owned());
    }
    for limitation in &report.limitations {
        lines.push(format!("Limitation: {limitation}"));
    }
    lines.push(format!("Showing {} of {} eligible context items; output truncated: {}; input paths omitted from retrieval: {}.", report.suggestions.len(), report.matched_count, report.truncated, report.omitted_input_paths));
    lines.push(format!("Content coverage: current truncated {}; omitted files {}; omitted local bases {}; omitted signals {}; historical scan truncated {}; oversized historical hunks omitted {}.", !report.input.content_omissions.is_empty(), report.input.content_omissions.len(), report.omitted_content_bases, report.omitted_content_signals, report.historical_content_truncated, report.omitted_historical_hunks));
    for item in &report.input.content_omissions {
        lines.push(format!(
            "  Content omitted: {} ({})",
            escape::path(&item.path),
            item.reason
        ));
    }
    if report.suggestions.is_empty() {
        lines.push("No sufficiently supported context items selected.".to_owned());
    }
    for suggestion in &report.suggestions {
        lines.push(format!(
            "{}: {}",
            suggestion.category.as_str(),
            escape::path(&suggestion.path)
        ));
        lines.push(format!(
            "  Associated current paths: {}",
            suggestion
                .associated_current_paths
                .iter()
                .map(|path| escape::path(path))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        lines.push(format!(
            "  Selection routes: {}",
            suggestion.selection_routes.join(", ")
        ));
        for basis in &suggestion.basis {
            lines.push(format!("  Basis: {}", escape::subject(basis)));
        }
        for citation in &suggestion.citations {
            lines.push(format!(
                "  Citation: {} {}",
                citation.oid,
                escape::subject(&citation.subject)
            ));
        }
        for item in &suggestion.content_matches {
            lines.push(format!("  Content: current {} {} (old {}, new {}) -> historical {} {} line {} (old {}, new {}); signals: {}",
                escape::path(&item.current_path), if item.current_added { "added" } else { "removed" },
                item.current_old_start, item.current_new_start, escape::path(&item.historical_path),
                if item.historical_added { "added" } else { "removed" }, item.historical_line,
                item.historical_old_start, item.historical_new_start, item.signals.iter().map(|s| escape::subject(s)).collect::<Vec<_>>().join(", ")));
            lines.push(format!(
                "    {}{}",
                escape::code_line(&item.excerpt),
                if item.excerpt_truncated {
                    " [excerpt truncated]"
                } else {
                    ""
                }
            ));
        }
        if suggestion.content_matches_truncated {
            lines.push("  Content matches truncated.".to_owned());
        }
        lines.push(format!(
            "  Supporting commits: {}; citations truncated: {}",
            suggestion.supporting_count, suggestion.citations_truncated
        ));
    }
    lines.join("\n")
}

pub(super) fn format_json_report(
    report: &ContextReport,
    warnings: &[String],
) -> Result<String, serde_json::Error> {
    serde_json::to_string(&JsonReport {
        schema_version: 1,
        kind: "context",
        input: JsonInput {
            mode: if report.input.staged {
                "staged"
            } else {
                "worktree"
            },
            head: report.input.head.as_deref(),
            changes: report
                .input
                .changes
                .iter()
                .map(|change| JsonChange {
                    status: &change.status,
                    old_path: change.old_path.as_deref().map(json::json_path),
                    new_path: change.new_path.as_deref().map(json::json_path),
                })
                .collect(),
        },
        cache_tip: report.cache_tip.as_deref(),
        scope: report.scope.as_ref().map(json::json_scope),
        historical_eligibility: if report.cache_tip.is_none() {
            "not_queried"
        } else if report.scope.is_some() {
            "scoped_published_cache"
        } else {
            "available_published_cache"
        },
        suggestions: report
            .suggestions
            .iter()
            .map(JsonSuggestion::from)
            .collect(),
        matched_count: report.matched_count,
        truncated: report.truncated,
        omitted_input_paths: report.omitted_input_paths,
        omitted_content_bases: report.omitted_content_bases,
        omitted_content_signals: report.omitted_content_signals,
        historical_content_truncated: report.historical_content_truncated,
        omitted_historical_hunks: report.omitted_historical_hunks,
        current_content_truncated: !report.input.content_omissions.is_empty(),
        omitted_current_files: report.input.content_omissions.len(),
        content_omissions: report
            .input
            .content_omissions
            .iter()
            .map(|item| JsonContentOmission {
                path: json::json_path(&item.path),
                reason: item.reason,
            })
            .collect(),
        limitations: &report.limitations,
        warnings: warnings.iter().chain(&report.warnings).collect(),
    })
}

#[derive(Serialize)]
struct JsonReport<'a> {
    schema_version: u8,
    kind: &'static str,
    input: JsonInput<'a>,
    cache_tip: Option<&'a str>,
    scope: Option<JsonSearchScope<'a>>,
    historical_eligibility: &'static str,
    suggestions: Vec<JsonSuggestion<'a>>,
    matched_count: usize,
    truncated: bool,
    omitted_input_paths: usize,
    omitted_content_bases: usize,
    omitted_content_signals: usize,
    historical_content_truncated: bool,
    omitted_historical_hunks: usize,
    current_content_truncated: bool,
    omitted_current_files: usize,
    content_omissions: Vec<JsonContentOmission<'a>>,
    limitations: &'a [String],
    warnings: Vec<&'a String>,
}
#[derive(Serialize)]
struct JsonContentOmission<'a> {
    path: JsonPath<'a>,
    reason: &'static str,
}
#[derive(Serialize)]
struct JsonInput<'a> {
    mode: &'static str,
    head: Option<&'a str>,
    changes: Vec<JsonChange<'a>>,
}
#[derive(Serialize)]
struct JsonChange<'a> {
    status: &'a str,
    old_path: Option<JsonPath<'a>>,
    new_path: Option<JsonPath<'a>>,
}
#[derive(Serialize)]
struct JsonSuggestion<'a> {
    category: &'static str,
    path: JsonPath<'a>,
    associated_current_paths: Vec<JsonPath<'a>>,
    basis: &'a [String],
    selection_routes: &'a [&'static str],
    citations: Vec<JsonCitation<'a>>,
    supporting_count: usize,
    citations_truncated: bool,
    content_matches: Vec<JsonContentMatch<'a>>,
    content_matches_truncated: bool,
}
#[derive(Serialize)]
struct JsonContentMatch<'a> {
    current_path: JsonPath<'a>,
    current_direction: &'static str,
    current_old_start: usize,
    current_new_start: usize,
    historical_path: JsonPath<'a>,
    historical_direction: &'static str,
    historical_line: usize,
    historical_old_start: i64,
    historical_new_start: i64,
    signals: &'a [String],
    excerpt: JsonPath<'a>,
    excerpt_truncated: bool,
}
#[derive(Serialize)]
struct JsonCitation<'a> {
    oid: &'a str,
    subject: &'a str,
}
impl<'a> From<&'a ContextSuggestion> for JsonSuggestion<'a> {
    fn from(suggestion: &'a ContextSuggestion) -> Self {
        Self {
            category: suggestion.category.as_str(),
            path: json::json_path(&suggestion.path),
            associated_current_paths: suggestion
                .associated_current_paths
                .iter()
                .map(|path| json::json_path(path))
                .collect(),
            basis: &suggestion.basis,
            selection_routes: &suggestion.selection_routes,
            citations: suggestion
                .citations
                .iter()
                .map(|citation| JsonCitation {
                    oid: &citation.oid,
                    subject: &citation.subject,
                })
                .collect(),
            supporting_count: suggestion.supporting_count,
            citations_truncated: suggestion.citations_truncated,
            content_matches_truncated: suggestion.content_matches_truncated,
            content_matches: suggestion
                .content_matches
                .iter()
                .map(|item| JsonContentMatch {
                    current_path: json::json_path(&item.current_path),
                    current_direction: if item.current_added {
                        "added"
                    } else {
                        "removed"
                    },
                    current_old_start: item.current_old_start,
                    current_new_start: item.current_new_start,
                    historical_path: json::json_path(&item.historical_path),
                    historical_direction: if item.historical_added {
                        "added"
                    } else {
                        "removed"
                    },
                    historical_line: item.historical_line,
                    historical_old_start: item.historical_old_start,
                    historical_new_start: item.historical_new_start,
                    signals: &item.signals,
                    excerpt: json::json_path(&item.excerpt),
                    excerpt_truncated: item.excerpt_truncated,
                })
                .collect(),
        }
    }
}
