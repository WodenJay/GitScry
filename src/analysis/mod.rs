//! Turning cache rows into traceable `material` for one capability.
//!
//! [`query`] owns the published generation, pinned target, scope semantics and
//! optional excerpts behind one execution interface. [`retrieval`] reads and
//! scores history; [`provenance`] interprets failure provenance; [`capabilities`]
//! assembles material. Ranking mechanics stay private; wording belongs to `render`.

mod capabilities;
mod patch;
mod provenance;
pub(crate) mod query;
mod retrieval;

pub(crate) use capabilities::timeline::{Entry as TimelineEntry, Report as TimelineReport};
pub(crate) use patch::{PatchExcerpt, PatchHunk, PatchStatus};
use retrieval::Intent;
pub(in crate::analysis) use retrieval::anchors_overlap;
pub(crate) use retrieval::{message_parts, normalize_path, searchable_text};

/// The complete `material` one capability returns.
pub(crate) struct Report {
    pub(crate) kind: ReportKind,
    pub(crate) materials: Vec<Material>,
    pub(crate) code_matches: Vec<CodeMatch>,
    pub(crate) matched_count: usize,
    pub(crate) truncated: bool,
    pub(crate) patch_mode: bool,
    pub(crate) warnings: Vec<String>,
    pub(crate) notices: Vec<String>,
    pub(crate) why: Option<Box<WhySummary>>,
    pub(crate) scope: Option<SearchScopeInfo>,
    pub(crate) symbol_summary: Option<SymbolSummary>,
}
#[derive(Clone, Debug)]
pub(crate) struct SearchScopeInfo {
    pub(crate) from_rev: Option<String>,
    pub(crate) to_rev: String,
    pub(crate) target_rev: Option<String>,
    pub(crate) since: Option<String>,
    pub(crate) until: Option<String>,
    pub(crate) cache_tip: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReportKind {
    Search,
    CodeSearch,
    Examples,
    Failures,
    Related,
    Tests,
    Why,
    Regression,
    TraceFix,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum CodeDirection {
    Added,
    Removed,
}

impl CodeDirection {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Removed => "removed",
        }
    }
}

/// An exact changed-line match, with the historical path from that line's side.
pub(crate) struct CodeMatch {
    pub(crate) commit_id: String,
    pub(crate) path: Vec<u8>,
    pub(crate) direction: CodeDirection,
    pub(crate) line_number: usize,
    pub(crate) line: Vec<u8>,
}

/// One result: the analogous or abandoned change, why it was selected, and its citations.
pub(crate) struct Material {
    pub(crate) subject: String,
    pub(crate) paths: Vec<Vec<u8>>,
    pub(crate) confidence: Confidence,
    pub(crate) basis: Vec<String>,
    /// The subject commit first, then every commit that supports this material.
    pub(crate) citations: Vec<Citation>,
    pub(crate) detail: Option<Detail>,
    pub(crate) patch: Option<PatchExcerpt>,
}

/// Capability-specific material beyond the shared citation/basis shape.
pub(crate) enum Detail {
    /// Moves history demonstrated, offered as precedent rather than instruction.
    Steps(Vec<Step>),
    /// The failure provenance history records for an abandoned approach.
    Failure(Failure),
    /// Co-change support for a candidate path.
    Relation(Relation),
    /// The fix, introducing change, and deleted-line evidence behind trace-fix.
    TraceFix(TraceFixDetail),
}

/// The separate facts returned by a line or symbol `why` query.
pub(crate) struct WhySummary {
    pub(crate) anchor: String,
    pub(crate) anchor_kind: &'static str,
    pub(crate) anchor_line: Option<usize>,
    pub(crate) symbol_end: Option<usize>,
    pub(crate) attribution_scope: &'static str,
    pub(crate) revision: String,
    pub(crate) target_related_modifications: Vec<WhyModification>,
    pub(crate) attribution: WhyAttribution,
    pub(crate) standalone_target_related_modification_count: usize,
    pub(crate) other_file_history_count: usize,
    pub(crate) file_history_count: usize,
    pub(crate) omitted_target_related_modifications: usize,
    pub(crate) timeline_follow_up_args: Option<Vec<String>>,
    pub(crate) limitations: Vec<String>,
}

pub(crate) struct WhyModification {
    pub(crate) oid: String,
    pub(crate) subject: String,
    pub(crate) paths: Vec<Vec<u8>>,
    pub(crate) basis: Vec<String>,
    pub(crate) patch: Option<PatchExcerpt>,
}

pub(crate) enum WhyAttribution {
    Available(WhyAttributionCommit),
    OutsideHistoricalScope,
    Unavailable { reason: String },
}

pub(crate) struct WhyAttributionCommit {
    pub(crate) oid: String,
    pub(crate) subject: String,
    pub(crate) shallow_boundary: bool,
    pub(crate) consolidated_target_modification: bool,
    pub(crate) basis: Vec<String>,
    pub(crate) patch: Option<PatchExcerpt>,
}

pub(crate) struct SymbolSummary {
    pub(crate) target: String,
    pub(crate) introduction: SymbolFact,
    pub(crate) anchor_line_attribution: SymbolFact,
}

pub(crate) enum SymbolFact {
    Known { commit_oid: String, subject: String },
    Unknown { reason: String },
}

pub(crate) struct TraceFixPatchAnchor {
    pub(crate) line: usize,
    pub(crate) paths: Vec<Vec<u8>>,
}

pub(crate) struct TraceFixDetail {
    pub(crate) role: &'static str,
    pub(crate) fix_revision: String,
    pub(crate) parent_revision: Option<String>,
    pub(crate) line: Option<usize>,
    pub(crate) patch_anchors: Vec<TraceFixPatchAnchor>,
}

/// One move a historical change made. Paths stay raw bytes for lossless rendering.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Step {
    Removed(Vec<u8>),
    Moved(Vec<u8>, Vec<u8>),
    Added(Vec<u8>),
    Modified(Vec<u8>),
}

impl Step {
    pub(crate) fn from_change(
        status: &str,
        old_path: Option<Vec<u8>>,
        new_path: Option<Vec<u8>>,
    ) -> Option<Self> {
        match (status.as_bytes().first(), old_path, new_path) {
            (Some(b'D'), Some(path), _) => Some(Self::Removed(path)),
            (Some(b'R' | b'C'), Some(old), Some(new)) => Some(Self::Moved(old, new)),
            (Some(b'A'), _, Some(path)) => Some(Self::Added(path)),
            (Some(b'M' | b'T'), _, Some(path)) => Some(Self::Modified(path)),
            _ => None,
        }
    }
}

/// The co-change facts rendered for a related path or test candidate.
pub(crate) struct Relation {
    pub(crate) co_change_count: usize,
    pub(crate) proportion: f64,
    pub(crate) supporting_count: usize,
}

/// What history records about why an approach failed. Absent text stays absent, so
/// rendering can print `Reason unknown` rather than inventing a cause.
pub(crate) struct Failure {
    pub(crate) reason: Option<String>,
    pub(crate) retry: Option<String>,
}

/// Strength of the supporting facts behind a piece of material.
pub(crate) enum Confidence {
    High,
    Medium,
    Low,
}

impl Confidence {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

/// A commit cited by a result, tagged with the role it plays in the material.
pub(crate) struct Citation {
    pub(crate) oid: String,
    pub(crate) abbreviation: String,
    pub(crate) subject: String,
    pub(crate) note: Option<&'static str>,
}

impl Citation {
    pub(crate) fn new(oid: String, subject: String) -> Self {
        Self {
            oid,
            abbreviation: String::new(),
            subject,
            note: None,
        }
    }

    pub(crate) fn noting(mut self, note: &'static str) -> Self {
        self.note = Some(note);
        self
    }
}

/// Report assembly, shared by the capabilities so truncation stays uniform.
pub(crate) fn report(
    kind: ReportKind,
    materials: Vec<Material>,
    matched_count: usize,
    limit: usize,
) -> Report {
    Report {
        kind,
        materials,
        code_matches: Vec::new(),
        matched_count,
        truncated: matched_count > limit,
        warnings: Vec::new(),
        notices: Vec::new(),
        why: None,
        scope: None,
        patch_mode: false,
        symbol_summary: None,
    }
}

pub(crate) fn empty_report(kind: ReportKind) -> Report {
    Report {
        kind,
        materials: Vec::new(),
        code_matches: Vec::new(),
        matched_count: 0,
        truncated: false,
        warnings: Vec::new(),
        notices: Vec::new(),
        why: None,
        scope: None,
        patch_mode: false,
        symbol_summary: None,
    }
}
