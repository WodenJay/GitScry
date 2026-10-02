use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Serialize;

use crate::analysis::{
    Citation, CodeMatch, Confidence, Detail, Failure, Material, PatchExcerpt, PatchHunk, Relation,
    Report, ReportKind, SearchScopeInfo, Step, SymbolFact, SymbolSummary,
};

const SCHEMA_VERSION: u8 = 1;

pub(crate) fn format_json_report(
    report: &Report,
    additional_warnings: &[String],
    github_links: Option<&crate::github::LinksReport>,
) -> Result<String, serde_json::Error> {
    let warnings = additional_warnings.iter().chain(&report.warnings).collect();
    serde_json::to_string(&JsonReport {
        schema_version: if github_links.is_some() {
            3
        } else if report.patch_mode {
            2
        } else {
            SCHEMA_VERSION
        },
        kind: report_kind(report.kind),
        matched_count: report.matched_count,
        truncated: report.truncated,
        materials: report.materials.iter().map(json_material).collect(),
        code_matches: (report.kind == ReportKind::CodeSearch)
            .then(|| report.code_matches.iter().map(json_code_match).collect()),
        warnings,
        notices: &report.notices,
        github_links,
        scope: report.scope.as_ref().map(json_scope),
        symbol_summary: report.symbol_summary.as_ref().map(json_symbol_summary),
    })
}

#[derive(Serialize)]
struct JsonReport<'a> {
    schema_version: u8,
    kind: &'static str,
    matched_count: usize,
    truncated: bool,
    materials: Vec<JsonMaterial<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code_matches: Option<Vec<JsonCodeMatch<'a>>>,
    warnings: Vec<&'a String>,
    notices: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    github_links: Option<&'a crate::github::LinksReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<JsonSearchScope<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol_summary: Option<JsonSymbolSummary<'a>>,
}

#[derive(Serialize)]
struct JsonSymbolSummary<'a> {
    introduction: JsonSymbolFact<'a>,
    target: &'a str,
    anchor_line_attribution: JsonSymbolFact<'a>,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum JsonSymbolFact<'a> {
    Known {
        commit_oid: &'a str,
        subject: &'a str,
    },
    Unknown {
        reason: &'a str,
    },
}

fn json_symbol_summary(summary: &SymbolSummary) -> JsonSymbolSummary<'_> {
    JsonSymbolSummary {
        target: &summary.target,
        introduction: json_symbol_fact(&summary.introduction),
        anchor_line_attribution: json_symbol_fact(&summary.anchor_line_attribution),
    }
}

fn json_symbol_fact(fact: &SymbolFact) -> JsonSymbolFact<'_> {
    match fact {
        SymbolFact::Known {
            commit_oid,
            subject,
        } => JsonSymbolFact::Known {
            commit_oid,
            subject,
        },
        SymbolFact::Unknown { reason } => JsonSymbolFact::Unknown { reason },
    }
}

#[derive(Serialize)]
pub(super) struct JsonSearchScope<'a> {
    from_rev: Option<&'a str>,
    to_rev: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_rev: Option<&'a str>,
    since: Option<&'a str>,
    until: Option<&'a str>,
    cache_tip: &'a str,
}

pub(super) fn json_scope(scope: &SearchScopeInfo) -> JsonSearchScope<'_> {
    JsonSearchScope {
        from_rev: scope.from_rev.as_deref(),
        to_rev: &scope.to_rev,
        target_rev: scope.target_rev.as_deref(),
        since: scope.since.as_deref(),
        until: scope.until.as_deref(),
        cache_tip: &scope.cache_tip,
    }
}

#[derive(Serialize)]
struct JsonMaterial<'a> {
    subject: &'a str,
    paths: Vec<JsonPath<'a>>,
    confidence: &'static str,
    basis: &'a [String],
    citations: Vec<JsonCitation<'a>>,
    detail: Option<JsonDetail<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    patch: Option<JsonPatch<'a>>,
}

#[derive(Serialize)]
pub(super) struct JsonPatch<'a> {
    commit_oid: &'a str,
    status: &'static str,
    hunks: Vec<JsonPatchHunk<'a>>,
    truncated: bool,
}

#[derive(Serialize)]
struct JsonPatchHunk<'a> {
    old_path: Option<JsonPath<'a>>,
    new_path: Option<JsonPath<'a>>,
    old_start: i64,
    old_lines: i64,
    new_start: i64,
    new_lines: i64,
    text: Option<JsonPath<'a>>,
    truncated: bool,
}

#[derive(Serialize)]
struct JsonCodeMatch<'a> {
    commit_id: &'a str,
    path: JsonPath<'a>,
    direction: &'static str,
    line_number: usize,
    line: JsonPath<'a>,
}

#[derive(Serialize)]
struct JsonCitation<'a> {
    oid: &'a str,
    abbreviation: &'a str,
    subject: &'a str,
    note: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(untagged)]
pub(super) enum JsonPath<'a> {
    Utf8(&'a str),
    Base64 { base64: String },
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum JsonDetail<'a> {
    Steps {
        steps: Vec<JsonStep<'a>>,
    },
    Failure {
        reason: &'a Option<String>,
        retry: &'a Option<String>,
    },
    Relation {
        co_change_count: usize,
        proportion: f64,
        supporting_count: usize,
    },
    Why {
        anchor: &'a str,
        revision: &'a str,
        line: usize,
    },
    TraceFix {
        role: &'a str,
        fix_revision: &'a str,
        parent_revision: &'a Option<String>,
        line: &'a Option<usize>,
    },
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum JsonStep<'a> {
    Removed {
        path: JsonPath<'a>,
    },
    Moved {
        old_path: JsonPath<'a>,
        new_path: JsonPath<'a>,
    },
    Added {
        path: JsonPath<'a>,
    },
    Modified {
        path: JsonPath<'a>,
    },
}

fn report_kind(kind: ReportKind) -> &'static str {
    match kind {
        ReportKind::Search => "search",
        ReportKind::Examples => "examples",
        ReportKind::CodeSearch => "code-search",
        ReportKind::Failures => "failures",
        ReportKind::Related => "related",
        ReportKind::Tests => "tests",
        ReportKind::Why => "why",
        ReportKind::Regression => "regression",
        ReportKind::TraceFix => "trace-fix",
    }
}

fn json_material(material: &Material) -> JsonMaterial<'_> {
    JsonMaterial {
        subject: &material.subject,
        paths: material.paths.iter().map(|path| json_path(path)).collect(),
        confidence: confidence(&material.confidence),
        basis: &material.basis,
        citations: material.citations.iter().map(json_citation).collect(),
        detail: material.detail.as_ref().map(json_detail),
        patch: material.patch.as_ref().map(json_patch),
    }
}

pub(super) fn json_patch(patch: &PatchExcerpt) -> JsonPatch<'_> {
    JsonPatch {
        commit_oid: &patch.commit_oid,
        status: patch.status.as_str(),
        hunks: patch.hunks.iter().map(json_patch_hunk).collect(),
        truncated: patch.truncated,
    }
}

fn json_patch_hunk(hunk: &PatchHunk) -> JsonPatchHunk<'_> {
    JsonPatchHunk {
        old_path: hunk.old_path.as_deref().map(json_path),
        new_path: hunk.new_path.as_deref().map(json_path),
        old_start: hunk.old_start,
        old_lines: hunk.old_lines,
        new_start: hunk.new_start,
        new_lines: hunk.new_lines,
        text: hunk.text.as_deref().map(json_path),
        truncated: hunk.truncated,
    }
}

fn json_code_match(matched: &CodeMatch) -> JsonCodeMatch<'_> {
    JsonCodeMatch {
        commit_id: &matched.commit_id,
        path: json_path(&matched.path),
        direction: matched.direction.as_str(),
        line_number: matched.line_number,
        line: json_path(&matched.line),
    }
}

fn json_citation(citation: &Citation) -> JsonCitation<'_> {
    JsonCitation {
        oid: &citation.oid,
        abbreviation: &citation.abbreviation,
        subject: &citation.subject,
        note: citation.note,
    }
}

pub(super) fn json_path(path: &[u8]) -> JsonPath<'_> {
    match std::str::from_utf8(path) {
        Ok(path) => JsonPath::Utf8(path),
        Err(_) => JsonPath::Base64 {
            base64: STANDARD.encode(path),
        },
    }
}

fn json_detail(detail: &Detail) -> JsonDetail<'_> {
    match detail {
        Detail::Steps(steps) => JsonDetail::Steps {
            steps: steps.iter().map(json_step).collect(),
        },
        Detail::Failure(Failure { reason, retry }) => JsonDetail::Failure { reason, retry },
        Detail::Relation(Relation {
            co_change_count,
            proportion,
            supporting_count,
        }) => JsonDetail::Relation {
            co_change_count: *co_change_count,
            proportion: *proportion,
            supporting_count: *supporting_count,
        },
        Detail::Why(why) => JsonDetail::Why {
            anchor: &why.anchor,
            revision: &why.revision,
            line: why.line,
        },
        Detail::TraceFix(trace) => JsonDetail::TraceFix {
            role: trace.role,
            fix_revision: &trace.fix_revision,
            parent_revision: &trace.parent_revision,
            line: &trace.line,
        },
    }
}

fn json_step(step: &Step) -> JsonStep<'_> {
    match step {
        Step::Removed(path) => JsonStep::Removed {
            path: json_path(path),
        },
        Step::Moved(old_path, new_path) => JsonStep::Moved {
            old_path: json_path(old_path),
            new_path: json_path(new_path),
        },
        Step::Added(path) => JsonStep::Added {
            path: json_path(path),
        },
        Step::Modified(path) => JsonStep::Modified {
            path: json_path(path),
        },
    }
}

fn confidence(confidence: &Confidence) -> &'static str {
    confidence.as_str()
}
