use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Serialize;

use crate::analysis::{
    Citation, Confidence, Detail, Failure, Material, Relation, Report, ReportKind, Step,
};

const SCHEMA_VERSION: u8 = 1;

pub(crate) fn format_json_report(report: &Report) -> Result<String, serde_json::Error> {
    serde_json::to_string(&JsonReport {
        schema_version: SCHEMA_VERSION,
        kind: report_kind(report.kind),
        matched_count: report.matched_count,
        truncated: report.truncated,
        materials: report.materials.iter().map(json_material).collect(),
        warnings: &report.warnings,
        notices: &report.notices,
    })
}

#[derive(Serialize)]
struct JsonReport<'a> {
    schema_version: u8,
    kind: &'static str,
    matched_count: usize,
    truncated: bool,
    materials: Vec<JsonMaterial<'a>>,
    warnings: &'a [String],
    notices: &'a [String],
}

#[derive(Serialize)]
struct JsonMaterial<'a> {
    subject: &'a str,
    paths: Vec<JsonPath<'a>>,
    confidence: &'static str,
    basis: &'a [String],
    citations: Vec<JsonCitation<'a>>,
    detail: Option<JsonDetail<'a>>,
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
enum JsonPath<'a> {
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

fn json_path(path: &[u8]) -> JsonPath<'_> {
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
