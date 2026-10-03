//! Cached reads grouped by responsibility. QuerySession owns the connection and generation.

mod abandonment;
mod commits;
mod patterns;
mod relations;
mod search;

pub(crate) use relations::RelationHistory;
pub(crate) use search::{SearchCandidate, SearchMaterial};

use super::cache_error;
use crate::app::AppError;

const SQL_PARAMETER_LIMIT: usize = 900;

fn search_error(operation: &str, error: impl std::fmt::Display) -> AppError {
    cache_error(operation, error)
}

fn numbered_placeholders(first: usize, count: usize) -> String {
    (first..first + count)
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ")
}
