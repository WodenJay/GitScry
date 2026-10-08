//! Execute queries against published history and assemble traceable material.
//!
//! `query::execute` is the execution interface. Capabilities own their feature
//! workflows; query internals share generation and scope semantics. Retrieval
//! scores history, provenance interprets failures, and render owns wording.

pub(crate) mod capabilities;
mod material;
mod patch;
mod patch_relationship;
mod provenance;
pub(crate) mod query;
mod retrieval;

pub(crate) use capabilities::context::{Report as ContextReport, Suggestion as ContextSuggestion};
pub(crate) use capabilities::hotspots::Report as HotspotsReport;
pub(crate) use capabilities::patterns::Report as PatternsReport;
pub(crate) use capabilities::timeline::{Entry as TimelineEntry, Report as TimelineReport};
pub(crate) use capabilities::trace_fix::TraceFixDetail;
pub(crate) use capabilities::trace_removal::FragmentReport as TraceRemovalFragmentReport;
pub(crate) use capabilities::trace_removal::Report as TraceRemovalReport;
pub(crate) use capabilities::why::{
    SymbolFact, SymbolSummary, WhyAttribution, WhyModification, WhySummary,
};
pub(crate) use material::{
    Citation, CodeDirection, CodeMatch, Confidence, Detail, Failure, Material, PatchEquivalence,
    PatchGroupMember, PatchGrouping, PatchIndeterminate, Relation, RelationSelector, Report,
    ReportKind, SearchScopeInfo, Step, empty_report, report,
};
pub(crate) use patch::{PatchExcerpt, PatchHunk, PatchStatus};
use retrieval::Intent;
pub(in crate::analysis) use retrieval::anchors_overlap;
pub(crate) use retrieval::follow_on::{
    MAX_PARENT_DISTANCE as FOLLOW_ON_MAX_PARENT_DISTANCE, PathObservation,
};
pub(crate) use retrieval::{message_parts, normalize_path, searchable_text};
