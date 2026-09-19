//! Assembling the material each query command returns.
//!
//! One reason to change: what a capability's answer is made of. Reading and scoring live
//! in [`super::retrieval`]; wording lives in `render`.

mod examples;
mod failures;
mod search;

use super::ReportKind;

pub(crate) use examples::run as examples;
pub(crate) use failures::run as failures;
pub(crate) use search::run as search;

/// The confidence a plain lexical result carries.
pub(super) fn confidence(signals: &super::retrieval::Signals) -> super::Confidence {
    if signals.strong() {
        super::Confidence::High
    } else if signals.moderate() {
        super::Confidence::Medium
    } else {
        super::Confidence::Low
    }
}

/// A report with no material and the capability's fixed empty answer.
pub(super) fn empty(kind: ReportKind) -> super::Report {
    super::empty_report(kind)
}
