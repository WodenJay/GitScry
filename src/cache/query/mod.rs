//! Cached reads grouped by responsibility. QuerySession owns the connection and generation.

mod abandonment;
mod commits;
mod patterns;
mod relations;
mod search;

pub(crate) use relations::RelationHistory;
pub(crate) use search::{SearchCandidate, SearchMaterial};

use super::{SQL_PARAMETER_LIMIT, cache_error};
use crate::app::AppError;

fn search_error(operation: &str, error: impl std::fmt::Display) -> AppError {
    cache_error(operation, error)
}

fn numbered_placeholders(first: usize, count: usize) -> String {
    (first..first + count)
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ")
}
