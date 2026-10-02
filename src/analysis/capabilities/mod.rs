//! Assembling the material each query command returns.
//!
//! One reason to change: what a capability returns. Reading and scoring live
//! in [`super::retrieval`]; wording lives in `render`.

mod code_search;
pub(crate) mod context;
mod context_abandonment;
mod context_content;
mod examples;
mod failures;
mod hybrid;
mod regression;
mod relations;
mod search;
pub(super) mod timeline;
mod trace_fix;
mod why;

pub(crate) use code_search::run as code_search;
pub(crate) use code_search::run_scoped as code_search_scoped;
pub(crate) use examples::run as examples;
pub(crate) use failures::run as failures;
pub(crate) use hybrid::run as hybrid_search;
pub(crate) use relations::{related, tests};
pub(crate) use search::run as search;
pub(crate) use search::run_scoped as search_scoped;

pub(crate) use regression::run as regression;
pub(crate) use trace_fix::run as trace_fix;
pub(crate) use why::run as why;
/// The confidence a result carries when nothing beyond the lexical match supports it.
pub(super) fn lexical_confidence(signals: &super::retrieval::Signals) -> super::Confidence {
    if signals.strong() {
        super::Confidence::High
    } else if signals.moderate() {
        super::Confidence::Medium
    } else {
        super::Confidence::Low
    }
}
