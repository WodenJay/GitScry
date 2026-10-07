//! Shared material, citations and report assembly.

use super::{PatchExcerpt, SymbolSummary, TraceFixDetail, WhySummary};

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
    pub(crate) target: Option<RelationTarget>,
    pub(crate) scope: Option<SearchScopeInfo>,
    pub(crate) fix_versions:
        Option<Box<crate::analysis::capabilities::trace_fix::TraceFixVersions>>,
    pub(crate) symbol_summary: Option<SymbolSummary>,
    pub(crate) symbol_selection: Option<Box<crate::git::SymbolSelection>>,
    pub(crate) relation_sources: Vec<super::capabilities::RelationSource>,
}
pub(crate) enum RelationSelector {
    Line {
        line: usize,
    },
    Symbol {
        name: String,
        start_line: usize,
        end_line: usize,
    },
}
pub(crate) struct RelationTarget {
    pub(crate) path: Vec<u8>,
    pub(crate) selector: RelationSelector,
    pub(crate) revision: String,
    pub(crate) status: &'static str,
    pub(crate) eligible_target_touch_commits: Option<usize>,
    pub(crate) limitations: Vec<String>,
}
#[derive(Clone, Debug)]
pub(crate) struct SearchScopeInfo {
    pub(crate) from_rev: Option<String>,
    pub(crate) to_rev: String,
    pub(crate) target_rev: Option<String>,
    pub(crate) since: Option<String>,
    pub(crate) until: Option<String>,
    /// Normalized QUERY-mode historical changed-path boundary; empty when the
    /// query did not restrict paths.
    pub(crate) paths: Vec<String>,
    pub(crate) cache_tip: String,
    pub(crate) coverage_complete: bool,
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
    pub(crate) follow_on: Vec<super::retrieval::follow_on::PathObservation>,
    pub(crate) co_change_citations: Vec<String>,
    pub(crate) module: Option<super::capabilities::ModuleCoChange>,
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
        relation_sources: Vec::new(),
        matched_count,
        truncated: matched_count > limit,
        warnings: Vec::new(),
        notices: Vec::new(),
        why: None,
        target: None,
        scope: None,
        fix_versions: None,
        patch_mode: false,
        symbol_summary: None,
        symbol_selection: None,
    }
}

pub(crate) fn empty_report(kind: ReportKind) -> Report {
    Report {
        kind,
        materials: Vec::new(),
        code_matches: Vec::new(),
        relation_sources: Vec::new(),
        matched_count: 0,
        truncated: false,
        warnings: Vec::new(),
        notices: Vec::new(),
        why: None,
        target: None,
        scope: None,
        fix_versions: None,
        patch_mode: false,
        symbol_summary: None,
        symbol_selection: None,
    }
}
