//! Assembling the material each query command returns.
//!
//! One reason to change: what a capability returns. Reading and scoring live
//! in [`super::retrieval`]; wording lives in `render`.

mod examples;
mod failures;
mod search;

pub(crate) use examples::run as examples;
pub(crate) use failures::run as failures;
pub(crate) use search::run as search;

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
