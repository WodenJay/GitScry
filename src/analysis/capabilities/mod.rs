//! Feature execution and material assembly.
//!
//! Each capability owns its workflow, target preparation and feature-specific
//! results. Published generation and scope semantics live in query internals;
//! reading and scoring live in retrieval; wording lives in render.

pub(in crate::analysis) mod code_search;
pub(crate) mod conflicts;
pub(crate) mod context;
mod examples;
mod failures;
pub(crate) mod followups;
pub(crate) mod fragment_search;
pub(crate) mod historical_conflicts;
pub(crate) mod hotspots;
pub(in crate::analysis) mod hybrid;
pub(crate) mod patterns;
pub(crate) mod propagation;
pub(in crate::analysis) mod regression;
pub(in crate::analysis) mod relations;
pub(in crate::analysis) mod search;
pub(crate) mod timeline;
pub(in crate::analysis) mod trace_fix;
pub(crate) mod trace_removal;
pub(crate) mod usage;
pub(in crate::analysis) mod why;

pub(crate) use examples::run as examples;
pub(crate) use failures::run as failures;
pub(crate) use relations::{ModuleCoChange, RelationSource};
pub(crate) use relations::{related, tests};

/// Normalize user-provided paths using Git's shared path canonicalizer.
fn normalize_git_path_string(path: &str) -> String {
    if path.starts_with('/') || path.starts_with('\\') {
        return path.replace('\\', "/");
    }
    String::from_utf8(crate::git::normalize_git_path(path.as_bytes()))
        .expect("normalizing a UTF-8 path preserves UTF-8")
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
