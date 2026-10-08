use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Serialize;

use crate::analysis::{
    Citation, CodeMatch, Confidence, Detail, Failure, Material, PatchEquivalence, PatchExcerpt,
    PatchGrouping, PatchHunk, Relation, RelationSelector, Report, ReportKind, SearchScopeInfo,
    Step, SymbolFact, SymbolSummary, WhyAttribution, WhyModification, WhySummary,
};

const SCHEMA_VERSION: u8 = 1;

pub(crate) fn format_json_report(
    report: &Report,
    additional_warnings: &[String],
    github_links: Option<&crate::github::LinksReport>,
) -> Result<String, serde_json::Error> {
    let warnings = additional_warnings.iter().chain(&report.warnings).collect();
    serde_json::to_string(&JsonReport {
        schema_version: if report.kind == ReportKind::Why {
            5
        } else if github_links.is_some() {
            4
        } else if report.patch_mode {
            2
        } else {
            SCHEMA_VERSION
        },
        kind: report_kind(report.kind),
        matched_count: report.matched_count,
        truncated: report.truncated,
        patch_grouping: report.patch_grouping.as_ref(),
        materials: (report.kind != ReportKind::Why)
            .then(|| report.materials.iter().map(json_material).collect()),
        target_related_modifications: report.why.as_ref().map(|why| {
            why.target_related_modifications
                .iter()
                .map(json_why_modification)
                .collect()
        }),
        why: report.why.as_ref().map(|why| json_why(why)),
        code_matches: (report.kind == ReportKind::CodeSearch)
            .then(|| report.code_matches.iter().map(json_code_match).collect()),
        warnings,
        notices: &report.notices,
        github_links,
        scope: report.scope.as_ref().map(json_scope),
        symbol_summary: report.symbol_summary.as_ref().map(json_symbol_summary),
        symbol_selection: report.symbol_selection.as_deref(),
        sources: &report.relation_sources,
        target: report.target.as_ref().map(|target| {
            let line = match &target.selector {
                RelationSelector::Line { line } => *line,
                RelationSelector::Symbol { start_line, .. } => *start_line,
            };
            let selector = match &target.selector {
                RelationSelector::Line { line } => JsonRelationSelector::Line { line: *line },
                RelationSelector::Symbol {
                    name,
                    start_line,
                    end_line,
                } => JsonRelationSelector::Symbol {
                    name,
                    start_line: *start_line,
                    end_line: *end_line,
                },
            };
            JsonRelationTarget {
                path: json_path(&target.path),
                line,
                selector,
                revision: &target.revision,
                status: target.status,
                eligible_target_touch_commits: target.eligible_target_touch_commits,
                limitations: &target.limitations,
            }
        }),
    })
}

#[derive(Serialize)]
struct JsonReport<'a> {
    schema_version: u8,
    kind: &'static str,
    matched_count: usize,
    truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    patch_grouping: Option<&'a PatchGrouping>,
    #[serde(skip_serializing_if = "Option::is_none")]
    materials: Option<Vec<JsonMaterial<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_related_modifications: Option<Vec<JsonWhyModification<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    why: Option<JsonWhy<'a>>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol_selection: Option<&'a crate::git::SymbolSelection>,
    #[serde(skip_serializing_if = "<[crate::analysis::capabilities::RelationSource]>::is_empty")]
    sources: &'a [crate::analysis::capabilities::RelationSource],
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<JsonRelationTarget<'a>>,
}

#[derive(Serialize)]
struct JsonModule<'a> {
    touch_commits: usize,
    support: Vec<JsonModuleSupport<'a>>,
}

#[derive(Serialize)]
struct JsonModuleSupport<'a> {
    oid: &'a str,
    paths: Vec<JsonPath<'a>>,
    sources: Vec<JsonModuleSource<'a>>,
}

#[derive(Serialize)]
struct JsonModuleSource<'a> {
    path: &'a str,
    kind: &'static str,
    paths: Vec<JsonPath<'a>>,
}
#[derive(Serialize)]
struct JsonRelationTarget<'a> {
    path: JsonPath<'a>,
    line: usize,
    selector: JsonRelationSelector<'a>,
    revision: &'a str,
    status: &'a str,
    eligible_target_touch_commits: Option<usize>,
    limitations: &'a [String],
}
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum JsonRelationSelector<'a> {
    Line {
        line: usize,
    },
    Symbol {
        name: &'a str,
        start_line: usize,
        end_line: usize,
    },
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
struct JsonWhy<'a> {
    anchor: JsonWhyAnchor<'a>,
    attribution_scope: &'a str,
    revision: &'a str,
    attribution: JsonWhyAttribution<'a>,
    counts: JsonWhyCounts,
    omitted_target_related_modifications: usize,
    timeline_follow_up: Option<JsonWhyTimelineFollowUp<'a>>,
    limitations: &'a [String],
}

#[derive(Serialize)]
struct JsonWhyAnchor<'a> {
    kind: &'static str,
    label: &'a str,
    line: Option<usize>,
    end_line: Option<usize>,
}

#[derive(Serialize)]
struct JsonWhyAttribution<'a> {
    state: &'static str,
    scope: &'a str,
    reason: Option<&'a str>,
    commit: Option<JsonWhyAttributionCommit<'a>>,
}

#[derive(Serialize)]
struct JsonWhyAttributionCommit<'a> {
    oid: &'a str,
    subject: &'a str,
    shallow_boundary: bool,
    consolidated_target_modification: bool,
    basis: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    patch: Option<JsonPatch<'a>>,
}

#[derive(Serialize)]
struct JsonWhyCounts {
    attribution: usize,
    standalone_target_related_modifications: usize,
    other_file_history: usize,
    file_history: usize,
}

#[derive(Serialize)]
struct JsonWhyTimelineFollowUp<'a> {
    program: &'static str,
    args: &'a [String],
}

#[derive(Serialize)]
struct JsonWhyModification<'a> {
    oid: &'a str,
    subject: &'a str,
    paths: Vec<JsonPath<'a>>,
    basis: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    patch: Option<JsonPatch<'a>>,
}

#[derive(Serialize)]
pub(super) struct JsonSearchScope<'a> {
    from_rev: Option<&'a str>,
    to_rev: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_rev: Option<&'a str>,
    since: Option<&'a str>,
    until: Option<&'a str>,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    paths: &'a [String],
    cache_tip: &'a str,
    coverage_complete: bool,
}

pub(super) fn json_scope(scope: &SearchScopeInfo) -> JsonSearchScope<'_> {
    JsonSearchScope {
        from_rev: scope.from_rev.as_deref(),
        to_rev: &scope.to_rev,
        target_rev: scope.target_rev.as_deref(),
        since: scope.since.as_deref(),
        until: scope.until.as_deref(),
        paths: &scope.paths,
        cache_tip: &scope.cache_tip,
        coverage_complete: scope.coverage_complete,
    }
}

fn json_why(why: &WhySummary) -> JsonWhy<'_> {
    let (state, reason, commit) = match &why.attribution {
        WhyAttribution::Available(attribution) => (
            "available",
            None,
            Some(JsonWhyAttributionCommit {
                oid: &attribution.oid,
                subject: &attribution.subject,
                shallow_boundary: attribution.shallow_boundary,
                consolidated_target_modification: attribution.consolidated_target_modification,
                basis: &attribution.basis,
                patch: attribution.patch.as_ref().map(json_patch),
            }),
        ),
        WhyAttribution::OutsideHistoricalScope => (
            "outside_historical_scope",
            Some("Target-line blame is outside the requested historical scope."),
            None,
        ),
        WhyAttribution::Unavailable { reason } => ("unavailable", Some(reason.as_str()), None),
    };
    JsonWhy {
        anchor: JsonWhyAnchor {
            kind: why.anchor_kind,
            label: &why.anchor,
            line: why.anchor_line,
            end_line: why.symbol_end,
        },
        attribution_scope: why.attribution_scope,
        revision: &why.revision,
        attribution: JsonWhyAttribution {
            state,
            scope: why.attribution_scope,
            reason,
            commit,
        },
        counts: JsonWhyCounts {
            attribution: usize::from(matches!(&why.attribution, WhyAttribution::Available(_))),
            standalone_target_related_modifications: why
                .standalone_target_related_modification_count,
            other_file_history: why.other_file_history_count,
            file_history: why.file_history_count,
        },
        omitted_target_related_modifications: why.omitted_target_related_modifications,
        timeline_follow_up: why.timeline_follow_up_args.as_deref().map(|args| {
            JsonWhyTimelineFollowUp {
                program: "gitscry",
                args,
            }
        }),
        limitations: &why.limitations,
    }
}

fn json_why_modification(modification: &WhyModification) -> JsonWhyModification<'_> {
    JsonWhyModification {
        oid: &modification.oid,
        subject: &modification.subject,
        paths: modification
            .paths
            .iter()
            .map(|path| json_path(path))
            .collect(),
        basis: &modification.basis,
        patch: modification.patch.as_ref().map(json_patch),
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
pub(super) struct JsonFollowOn<'a> {
    source: JsonPath<'a>,
    candidate: JsonPath<'a>,
    supporting_origins: usize,
    eligible_origins: usize,
    independent_chains: usize,
    baseline_occurrences: usize,
    baseline_sample_size: usize,
    observation_days: usize,
    max_parent_distance: usize,
    bounds_inclusive: bool,
    proper_descendants_only: bool,
    merge_events_counted: bool,
    omitted_examples: usize,
    denominator_basis: &'static str,
    limitations: &'static str,
    examples: Vec<JsonFollowOnExample<'a>>,
}

#[derive(Serialize)]
struct JsonFollowOnExample<'a> {
    origin_oid: &'a str,
    later_oid: &'a str,
    origin_path: JsonPath<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin_paths: Option<Vec<JsonPath<'a>>>,
    later_path: JsonPath<'a>,
    parent_distance: usize,
    elapsed_seconds: i64,
}

pub(super) fn json_follow_on(evidence: &crate::analysis::PathObservation) -> JsonFollowOn<'_> {
    let observation = &evidence.observation;
    JsonFollowOn {
        source: json_path(&evidence.source),
        candidate: json_path(&observation.path),
        supporting_origins: observation.supporting_origins,
        eligible_origins: observation.eligible_origins,
        independent_chains: observation.independent_chains,
        baseline_occurrences: observation.baseline_occurrences,
        baseline_sample_size: observation.baseline_sample_size,
        observation_days: observation.observation_days,
        max_parent_distance: crate::analysis::FOLLOW_ON_MAX_PARENT_DISTANCE,
        bounds_inclusive: true,
        proper_descendants_only: true,
        merge_events_counted: false,
        omitted_examples: observation.omitted_examples,
        denominator_basis: "complete non-merge origins excluding candidate-touching origins; background uniformly samples at most 100 such origins",
        limitations: "detected renames only; copies and recreation start new incarnations; incomplete or unobserved windows excluded; historical association, not causation",
        examples: observation
            .chains
            .iter()
            .map(|chain| JsonFollowOnExample {
                origin_oid: &chain.origin_oid,
                later_oid: &chain.later_oid,
                origin_path: json_path(&chain.origin_path),
                origin_paths: (chain.origin_paths.len() > 1).then(|| {
                    chain
                        .origin_paths
                        .iter()
                        .map(|path| json_path(path))
                        .collect()
                }),
                later_path: json_path(&chain.later_path),
                parent_distance: chain.parent_distance,
                elapsed_seconds: chain.elapsed_seconds,
            })
            .collect(),
    }
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum JsonDetail<'a> {
    Steps {
        steps: Vec<JsonStep<'a>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        patch_equivalence: Option<&'a PatchEquivalence>,
    },
    Failure {
        reason: &'a Option<String>,
        retry: &'a Option<String>,
    },
    Relation {
        co_change_count: usize,
        proportion: f64,
        supporting_count: usize,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        follow_on: Vec<JsonFollowOn<'a>>,
        co_change_citations: &'a [String],
        #[serde(skip_serializing_if = "Option::is_none")]
        module: Option<JsonModule<'a>>,
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
        Detail::Steps {
            steps,
            patch_equivalence,
        } => JsonDetail::Steps {
            steps: steps.iter().map(json_step).collect(),
            patch_equivalence: patch_equivalence.as_ref(),
        },
        Detail::Failure(Failure { reason, retry }) => JsonDetail::Failure { reason, retry },
        Detail::Relation(Relation {
            co_change_count,
            proportion,
            supporting_count,
            follow_on,
            co_change_citations,
            module,
        }) => JsonDetail::Relation {
            co_change_count: *co_change_count,
            proportion: *proportion,
            supporting_count: *supporting_count,
            follow_on: follow_on.iter().map(json_follow_on).collect(),
            co_change_citations,
            module: module.as_ref().map(|module| JsonModule {
                touch_commits: module.touch_commits,
                support: module
                    .support
                    .iter()
                    .map(|support| JsonModuleSupport {
                        oid: &support.oid,
                        paths: support.paths.iter().map(|path| json_path(path)).collect(),
                        sources: support
                            .sources
                            .iter()
                            .map(|source| JsonModuleSource {
                                path: &source.path,
                                kind: source.kind,
                                paths: source.paths.iter().map(|path| json_path(path)).collect(),
                            })
                            .collect(),
                    })
                    .collect(),
            }),
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
