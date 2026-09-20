//! Turning cache rows into traceable `material` for one capability.
//!
//! Three parts, three reasons to change. [`retrieval`] owns *how history is read and
//! scored*; [`provenance`] owns *how a commit message is read for failure provenance*;
//! [`capabilities`] owns *what material each command assembles*. Ranking weights and
//! retrieval mechanics stay private, and all user-visible wording belongs to `render`.

mod capabilities;
mod provenance;
mod retrieval;

use std::{collections::HashSet, path::Path};

use rusqlite::Connection;

use crate::app::AppError;
use crate::git::{TraceFixTarget, WhyTarget};

pub(crate) use retrieval::{Intent, ancestors, message_parts, searchable_text};

/// The complete `material` one capability returns.
pub(crate) struct Report {
    pub(crate) kind: ReportKind,
    pub(crate) materials: Vec<Material>,
    pub(crate) matched_count: usize,
    pub(crate) truncated: bool,
    pub(crate) warnings: Vec<String>,
    pub(crate) notices: Vec<String>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReportKind {
    Search,
    Examples,
    Failures,
    Related,
    Tests,
    Why,
    Regression,
    TraceFix,
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
}

/// Capability-specific material beyond the shared citation/basis shape.
pub(crate) enum Detail {
    /// Moves history demonstrated, offered as precedent rather than instruction.
    Steps(Vec<Step>),
    /// The failure provenance history records for an abandoned approach.
    Failure(Failure),
    /// Co-change support for a candidate path.
    Relation(Relation),
    /// The target anchor and revision behind a why explanation.
    Why(WhyDetail),
    /// The fix, introducing change, and deleted-line evidence behind trace-fix.
    TraceFix(TraceFixDetail),
}

pub(crate) struct WhyDetail {
    pub(crate) anchor: String,
    pub(crate) revision: String,
    pub(crate) line: usize,
}

pub(crate) struct TraceFixDetail {
    pub(crate) role: &'static str,
    pub(crate) fix_revision: String,
    pub(crate) parent_revision: Option<String>,
    pub(crate) line: Option<usize>,
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

pub(crate) fn search(
    connection: &Connection,
    intent: &Intent,
    limit: usize,
) -> Result<Report, AppError> {
    validate_limit(limit)?;
    capabilities::search(connection, intent, limit)
}

pub(crate) fn examples(
    connection: &Connection,
    intent: &Intent,
    limit: usize,
) -> Result<Report, AppError> {
    validate_limit(limit)?;
    capabilities::examples(connection, intent, limit)
}

pub(crate) fn why(
    connection: &Connection,
    target: &WhyTarget,
    reachable: &HashSet<String>,
    limit: usize,
) -> Result<Report, AppError> {
    validate_limit(limit)?;
    capabilities::why(connection, target, reachable, limit)
}

pub(crate) fn regression(
    connection: &Connection,
    intent: &Intent,
    target: &crate::git::RegressionTarget,
    reachable: &HashSet<String>,
    limit: usize,
) -> Result<Report, AppError> {
    validate_limit(limit)?;
    capabilities::regression(connection, intent, target, reachable, limit)
}

pub(crate) fn trace_fix(
    connection: &Connection,
    target: &TraceFixTarget,
    reachable: &HashSet<String>,
    limit: usize,
) -> Result<Report, AppError> {
    validate_limit(limit)?;
    capabilities::trace_fix(connection, target, reachable, limit)
}

pub(crate) fn failures(
    connection: &Connection,
    intent: &Intent,
    limit: usize,
) -> Result<Report, AppError> {
    validate_limit(limit)?;
    capabilities::failures(connection, intent, limit)
}

pub(crate) fn related(
    connection: &Connection,
    intent: &Intent,
    worktree_root: &Path,
    limit: usize,
) -> Result<Report, AppError> {
    validate_limit(limit)?;
    capabilities::related(connection, intent, worktree_root, limit)
}

pub(crate) fn tests(
    connection: &Connection,
    intent: &Intent,
    worktree_root: &Path,
    limit: usize,
) -> Result<Report, AppError> {
    validate_limit(limit)?;
    capabilities::tests(connection, intent, worktree_root, limit)
}

fn validate_limit(limit: usize) -> Result<(), AppError> {
    (limit > 0)
        .then_some(())
        .ok_or_else(|| AppError::input("limit must be greater than zero"))
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
        matched_count,
        truncated: matched_count > limit,
        warnings: Vec::new(),
        notices: Vec::new(),
    }
}

pub(crate) fn empty_report(kind: ReportKind) -> Report {
    Report {
        kind,
        materials: Vec::new(),
        matched_count: 0,
        truncated: false,
        warnings: Vec::new(),
        notices: Vec::new(),
    }
}

pub(crate) fn search_error(operation: &str, error: impl std::fmt::Display) -> AppError {
    AppError::operational(format!(
        "error: {operation}: {error}; delete .gitscry and retry"
    ))
}
