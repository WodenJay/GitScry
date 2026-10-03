//! Feature execution and material assembly.
//!
//! Each capability owns its workflow, target preparation and feature-specific
//! results. Published generation and scope semantics live in query internals;
//! reading and scoring live in retrieval; wording lives in render.

pub(in crate::analysis) mod code_search;
pub(crate) mod context;
mod examples;
mod failures;
pub(crate) mod followups;
pub(crate) mod hotspots;
pub(in crate::analysis) mod hybrid;
pub(crate) mod patterns;
pub(in crate::analysis) mod regression;
mod relations;
pub(in crate::analysis) mod search;
pub(crate) mod timeline;
pub(in crate::analysis) mod trace_fix;
pub(crate) mod trace_removal;
pub(crate) mod usage;
pub(in crate::analysis) mod why;

pub(crate) use examples::run as examples;
pub(crate) use failures::run as failures;
pub(crate) use relations::{related, tests};

/// Convert user-facing path separators to Git's canonical separator.
fn normalize_path_separators(path: &str) -> String {
    path.replace('\\', "/")
}

pub(super) fn lexical_confidence(signals: &super::retrieval::Signals) -> super::Confidence {
    if signals.strong() {
        super::Confidence::High
    } else if signals.moderate() {
        super::Confidence::Medium
    } else {
        super::Confidence::Low
    }
}
