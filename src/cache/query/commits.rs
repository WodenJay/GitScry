//! Commit messages, projected paths and ordered commit scans.

use rusqlite::{Connection, OptionalExtension, params_from_iter};
use std::collections::HashSet;

use super::search_error;
use crate::app::AppError;
use crate::cache::scope::{
    SEARCH_SCOPE_CTE, SearchFilter, changed_path_predicate, changed_path_values, scope_values,
};
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

    pub(crate) fn commits(&self) -> Result<Vec<StoredCommit>, AppError> {
        commits(&self.connection)
    }
    pub(crate) fn commits_reachable_from(&self, oid: &str) -> Result<Vec<String>, AppError> {
        commits_reachable_from(&self.connection, oid)
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

/// A cached commit: its object ID and message.
pub(crate) struct StoredCommit {
    pub(crate) oid: String,
    pub(crate) message: Vec<u8>,
}

/// Every cached commit, oldest first.
fn commits(connection: &Connection) -> Result<Vec<StoredCommit>, AppError> {
    let mut statement = connection
        .prepare("SELECT oid, message, message_length FROM commits ORDER BY position")
        .map_err(|error| search_error("preparing history scan", error))?;
    statement
        .query_map([], |row| {
            Ok(StoredCommit {
                oid: row.get(0)?,
                message: decode_message_row(row, 1, 2)?,
            })
        })
        .map_err(|error| search_error("reading history scan", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| search_error("reading history scan", error))
}

/// Cached commits reachable from one revision, oldest first.
fn commits_reachable_from(connection: &Connection, oid: &str) -> Result<Vec<String>, AppError> {
    let mut statement = connection
        .prepare(
            "WITH RECURSIVE reachable(commit_id) AS (
                SELECT commit_id FROM commits WHERE oid = ?1
                UNION
                SELECT parent.parent_id
                FROM commit_parents AS parent
                JOIN reachable AS child ON child.commit_id = parent.commit_id
                WHERE parent.parent_id IS NOT NULL
            )
            SELECT commits.oid
            FROM commits
            JOIN reachable ON reachable.commit_id = commits.commit_id
            ORDER BY commits.position",
        )
        .map_err(|error| search_error("preparing cached reachable history", error))?;
    statement
        .query_map([oid], |row| row.get(0))
        .map_err(|error| search_error("reading cached reachable history", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| search_error("reading cached reachable history", error))
}

/// Commits eligible under the same revision, committer-time, and changed-path bounds as scoped search.
fn commits_scoped(
    connection: &Connection,
    scope: &SearchFilter,
) -> Result<Vec<StoredCommit>, AppError> {
    let path_eligibility = changed_path_predicate(scope, 5);
    let query = format!(
        "{SEARCH_SCOPE_CTE}
         SELECT c.oid, c.message, c.message_length
         FROM commits AS c
         JOIN eligible ON eligible.commit_id = c.commit_id
         {path_eligibility}
         ORDER BY c.position"
    );
    let mut values = scope_values(scope).to_vec();
    values.extend(changed_path_values(scope));
    let mut statement = connection
        .prepare(&query)
        .map_err(|error| search_error("preparing scoped history scan", error))?;
    statement
        .query_map(params_from_iter(values), |row| {
            Ok(StoredCommit {
                oid: row.get(0)?,
                message: decode_message_row(row, 1, 2)?,
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

#[cfg(test)]
mod tests {
    use super::commits_scoped;
    use crate::cache::scope::SearchFilter;
    use rusqlite::Connection;

    #[test]
    fn scoped_commit_scan_respects_path_bounds() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE commits (
                    commit_id INTEGER PRIMARY KEY,
                    position INTEGER NOT NULL,
                    oid TEXT NOT NULL,
                    commit_time INTEGER NOT NULL,
                    message BLOB NOT NULL,
                    message_length INTEGER NOT NULL
                );
                CREATE TABLE commit_paths (commit_id INTEGER NOT NULL, raw_path BLOB NOT NULL);
                CREATE TEMP TABLE query_scope_revisions (role TEXT NOT NULL, oid TEXT NOT NULL);
                INSERT INTO query_scope_revisions VALUES ('reachable', 'binary');",
            )
            .unwrap();
        let (route_message, route_length) = crate::cache::payload::encode(b"route message");
        let (binary_message, binary_length) = crate::cache::payload::encode(b"binary message");
        connection
            .execute(
                "INSERT INTO commits VALUES (?1, ?2, ?3, 1, ?4, ?5)",
                rusqlite::params![1, 1, "route", route_message, route_length],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO commits VALUES (?1, ?2, ?3, 1, ?4, ?5)",
                rusqlite::params![2, 2, "binary", binary_message, binary_length],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO commit_paths VALUES (?1, ?2)",
                rusqlite::params![1, b"src/route.rs".as_slice()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO commit_paths VALUES (?1, ?2)",
                rusqlite::params![2, b"src/other.bin".as_slice()],
            )
            .unwrap();
        let scope = SearchFilter {
            from_oid: None,
            to_oid: "route".to_owned(),
            since: None,
            until: None,
            paths: vec!["src/route.rs".to_owned()],
        };

        let commits = commits_scoped(&connection, &scope).unwrap();

        assert_eq!(
            commits
                .iter()
                .map(|commit| commit.oid.as_str())
                .collect::<Vec<_>>(),
            ["route"]
        );
    }
}
