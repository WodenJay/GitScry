//! Commit messages, positions, projected paths and ordered commit scans.

use rusqlite::{Connection, OptionalExtension, params_from_iter};
use std::collections::HashSet;

use super::search_error;
use crate::app::AppError;
use crate::cache::scope::{SEARCH_SCOPE_CTE, SearchFilter, scope_values};
use crate::cache::{QuerySession, decode_message_row};

impl QuerySession {
    pub(crate) fn projected_paths(&self, oid: &str) -> Result<Vec<Vec<u8>>, AppError> {
        projected_paths(&self.connection, oid)
    }

    pub(crate) fn changes(&self, oid: &str) -> Result<Vec<StoredChange>, AppError> {
        changes(&self.connection, oid)
    }

    pub(crate) fn commit_message(&self, oid: &str) -> Result<Option<Vec<u8>>, AppError> {
        commit_message(&self.connection, oid)
    }

    pub(crate) fn commit_position(&self, oid: &str) -> Result<Option<i64>, AppError> {
        self.connection
            .query_row(
                "SELECT position FROM commits WHERE oid = ?1",
                [oid],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| search_error("reading commit position", error))
    }

    pub(crate) fn commits(&self) -> Result<Vec<StoredCommit>, AppError> {
        commits(&self.connection)
    }

    pub(crate) fn commits_scoped(
        &self,
        scope: &SearchFilter,
    ) -> Result<Vec<StoredCommit>, AppError> {
        commits_scoped(&self.connection, scope)
    }

    pub(crate) fn commit_oids(&self) -> Result<HashSet<String>, AppError> {
        commit_oids(&self.connection)
    }
}
pub(crate) struct StoredChange {
    pub(crate) status: String,
    pub(crate) old_path: Option<Vec<u8>>,
    pub(crate) new_path: Option<Vec<u8>>,
}
fn projected_paths(connection: &Connection, oid: &str) -> Result<Vec<Vec<u8>>, AppError> {
    let mut statement = connection
        .prepare(
            "SELECT cp.raw_path
             FROM commit_paths AS cp
             JOIN commits AS c ON c.commit_id = cp.commit_id
             WHERE c.oid = ?1
             ORDER BY cp.path_order",
        )
        .map_err(|error| search_error("preparing path projection", error))?;
    statement
        .query_map([oid], |row| row.get::<_, Vec<u8>>(0))
        .map_err(|error| search_error("reading path projection", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| search_error("reading path projection", error))
}

/// The raw path changes a commit made, in change order.
fn changes(connection: &Connection, oid: &str) -> Result<Vec<StoredChange>, AppError> {
    let mut statement = connection
        .prepare(
            "SELECT ch.status, ch.old_path, ch.new_path
             FROM changes AS ch
             JOIN commits AS c ON c.commit_id = ch.commit_id
             WHERE c.oid = ?1
             ORDER BY ch.ordinal",
        )
        .map_err(|error| search_error("preparing change shapes", error))?;
    statement
        .query_map([oid], |row| {
            Ok(StoredChange {
                status: row.get(0)?,
                old_path: row.get(1)?,
                new_path: row.get(2)?,
            })
        })
        .map_err(|error| search_error("reading change shapes", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| search_error("reading change shapes", error))
}

/// The decoded message of one cached commit.
fn commit_message(connection: &Connection, oid: &str) -> Result<Option<Vec<u8>>, AppError> {
    let mut statement = connection
        .prepare("SELECT message, message_length FROM commits WHERE oid = ?1")
        .map_err(|error| search_error("preparing commit lookup", error))?;
    let message = statement
        .query_row([oid], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?))
        })
        .optional()
        .map_err(|error| search_error("reading commit lookup", error))?;
    message
        .map(|(compressed, length)| crate::cache::decode_message(&compressed, length))
        .transpose()
}

/// A cached commit: its object ID, message, and cache position.
///
/// The position orders commits the way the cache generation was built, which separates
/// commits a repository recorded in the same second.
pub(crate) struct StoredCommit {
    pub(crate) position: i64,
    pub(crate) oid: String,
    pub(crate) message: Vec<u8>,
}

/// Every cached commit, oldest first.
fn commits(connection: &Connection) -> Result<Vec<StoredCommit>, AppError> {
    let mut statement = connection
        .prepare("SELECT position, oid, message, message_length FROM commits ORDER BY position")
        .map_err(|error| search_error("preparing history scan", error))?;
    statement
        .query_map([], |row| {
            Ok(StoredCommit {
                position: row.get(0)?,
                oid: row.get(1)?,
                message: decode_message_row(row, 2, 3)?,
            })
        })
        .map_err(|error| search_error("reading history scan", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| search_error("reading history scan", error))
}

/// Commits eligible under the same revision and committer-time bounds as scoped search.
fn commits_scoped(
    connection: &Connection,
    scope: &SearchFilter,
) -> Result<Vec<StoredCommit>, AppError> {
    let query = format!(
        "{SEARCH_SCOPE_CTE}
         SELECT c.position, c.oid, c.message, c.message_length
         FROM commits AS c
         JOIN eligible ON eligible.commit_id = c.commit_id
         ORDER BY c.position"
    );
    let mut statement = connection
        .prepare(&query)
        .map_err(|error| search_error("preparing scoped history scan", error))?;
    statement
        .query_map(params_from_iter(scope_values(scope)), |row| {
            Ok(StoredCommit {
                position: row.get(0)?,
                oid: row.get(1)?,
                message: decode_message_row(row, 2, 3)?,
            })
        })
        .map_err(|error| search_error("reading scoped history scan", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| search_error("reading scoped history scan", error))
}

fn commit_oids(connection: &Connection) -> Result<HashSet<String>, AppError> {
    let mut statement = connection
        .prepare("SELECT oid FROM commits")
        .map_err(|error| search_error("preparing cached object ID scan", error))?;
    statement
        .query_map([], |row| row.get(0))
        .map_err(|error| search_error("reading cached object IDs", error))?
        .collect::<Result<HashSet<_>, _>>()
        .map_err(|error| search_error("reading cached object IDs", error))
}
