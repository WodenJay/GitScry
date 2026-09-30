//! Writing a complete snapshot into cache rows and derived search projections.
//!
//! Transactions, payload encoding and row invariants stay behind build/append;
//! generation owns when those writes become the published cache.

use std::{collections::HashMap, path::Path};

use rusqlite::{Connection, OptionalExtension, params};

use super::{
    SCHEMA_VERSION, cache_error,
    payload::{HunkWriter, encode},
    schema,
};
use crate::{
    analysis,
    app::AppError,
    git::{Change, Commit, Hunk, PatchStream, Snapshot},
};

pub(super) fn append(path: &Path, snapshot: &Snapshot) -> Result<usize, AppError> {
    let mut connection =
        Connection::open(path).map_err(|error| cache_error("opening cache", error))?;
    connection
        .execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = FULL;")
        .map_err(|error| cache_error("configuring cache", error))?;
    let transaction = connection
        .transaction()
        .map_err(|error| cache_error("starting cache transaction", error))?;
    write_snapshot_rows(&transaction, snapshot, replace_commit)?;
    replace_metadata(&transaction, snapshot)?;
    transaction
        .execute("DELETE FROM shallow_boundaries", [])
        .map_err(|error| cache_error("updating shallow boundaries", error))?;
    for oid in &snapshot.shallow_boundaries {
        transaction
            .execute("INSERT INTO shallow_boundaries(oid) VALUES (?1)", [oid])
            .map_err(|error| cache_error("writing shallow boundary", error))?;
    }
    transaction
        .execute("DELETE FROM missing_objects", [])
        .map_err(|error| cache_error("updating missing objects", error))?;
    for oid in &snapshot.missing_objects {
        transaction
            .execute("INSERT INTO missing_objects(oid) VALUES (?1)", [oid])
            .map_err(|error| cache_error("writing missing object", error))?;
    }
    transaction
        .execute("INSERT INTO search_fts(search_fts) VALUES ('optimize')", [])
        .map_err(|error| cache_error("optimizing search index", error))?;
    let commit_count = count_connection(&transaction, "SELECT COUNT(*) FROM commits")?;
    validate_connection(&transaction, commit_count)?;
    transaction
        .execute(
            "UPDATE metadata SET value = ?1 WHERE key = 'completed_commit_count'",
            [commit_count.to_string()],
        )
        .map_err(|error| cache_error("updating completed commit count", error))?;
    transaction
        .commit()
        .map_err(|error| cache_error("committing cache update", error))?;
    usize::try_from(commit_count)
        .map_err(|_| cache_error("counting cached commits", "count exceeded platform limits"))
}

pub(super) fn build(path: &Path, snapshot: &Snapshot) -> Result<(), AppError> {
    let mut connection =
        Connection::open(path).map_err(|error| cache_error("opening staging cache", error))?;
    connection
        .execute_batch(schema::SCHEMA)
        .map_err(|error| cache_error("creating cache schema", error))?;
    let transaction = connection
        .transaction()
        .map_err(|error| cache_error("starting cache transaction", error))?;
    replace_metadata(&transaction, snapshot)?;
    write_snapshot_rows(&transaction, snapshot, insert_commit)?;
    for oid in &snapshot.shallow_boundaries {
        transaction
            .execute("INSERT INTO shallow_boundaries(oid) VALUES (?1)", [oid])
            .map_err(|error| cache_error("writing shallow boundary", error))?;
    }
    for oid in &snapshot.missing_objects {
        transaction
            .execute("INSERT INTO missing_objects(oid) VALUES (?1)", [oid])
            .map_err(|error| cache_error("writing missing object", error))?;
    }
    transaction
        .execute("INSERT INTO search_fts(search_fts) VALUES ('optimize')", [])
        .map_err(|error| cache_error("optimizing search index", error))?;
    transaction
        .commit()
        .map_err(|error| cache_error("committing staging cache", error))?;
    connection
        .close()
        .map_err(|(_, error)| cache_error("closing staging cache", error))
}

fn write_snapshot_rows(
    transaction: &rusqlite::Transaction<'_>,
    snapshot: &Snapshot,
    write_commit: fn(&Connection, &Commit) -> Result<(), AppError>,
) -> Result<(), AppError> {
    let change_ids = {
        let paths_by_commit = paths_by_commit(snapshot);
        let projected_paths = projected_paths_by_commit(snapshot);
        for commit in &snapshot.commits {
            write_commit(transaction, commit)?;
        }
        for commit in &snapshot.commits {
            insert_parents(transaction, commit)?;
            insert_document(
                transaction,
                commit,
                paths_by_commit
                    .get(&commit.oid)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            )?;
        }
        let change_ids = insert_changes(transaction, &snapshot.changes)?;
        for commit in &snapshot.commits {
            insert_path_projection(
                transaction,
                commit,
                projected_paths
                    .get(&commit.oid)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            )?;
        }
        change_ids
    };
    write_hunks(transaction, &snapshot.patches, &change_ids)
}

fn paths_by_commit(snapshot: &Snapshot) -> HashMap<String, Vec<Vec<u8>>> {
    let mut paths_by_commit = HashMap::<String, Vec<Vec<u8>>>::new();
    for change in &snapshot.changes {
        let paths = paths_by_commit
            .entry(change.commit_oid.clone())
            .or_default();
        for path in [&change.old_path, &change.new_path].into_iter().flatten() {
            if !paths.iter().any(|existing| existing == path) {
                paths.push(path.clone());
            }
        }
    }
    paths_by_commit
}

struct ProjectedPath {
    key: String,
    basename: String,
    raw_path: Vec<u8>,
    order: i64,
}

fn projected_paths_by_commit(snapshot: &Snapshot) -> HashMap<String, Vec<ProjectedPath>> {
    let mut paths_by_commit = HashMap::<String, Vec<ProjectedPath>>::new();
    for change in &snapshot.changes {
        let paths = paths_by_commit
            .entry(change.commit_oid.clone())
            .or_default();
        for path in [&change.old_path, &change.new_path].into_iter().flatten() {
            let key = analysis::normalize_path(path);
            if key.is_empty() || paths.iter().any(|existing| existing.key == key) {
                continue;
            }
            let basename = key.rsplit('/').next().unwrap_or_default().to_owned();
            paths.push(ProjectedPath {
                key,
                basename,
                raw_path: path.clone(),
                order: paths.len() as i64,
            });
        }
    }
    paths_by_commit
}
fn replace_metadata(connection: &Connection, snapshot: &Snapshot) -> Result<(), AppError> {
    for (key, value) in [
        ("schema_version", SCHEMA_VERSION.to_owned()),
        ("object_format", snapshot.object_format.clone()),
        ("default_ref", snapshot.default_ref.clone()),
        ("completed_tip", snapshot.tip.clone()),
        ("completed_commit_count", snapshot.commits.len().to_string()),
    ] {
        connection
            .execute(
                "INSERT INTO metadata(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(|error| cache_error("writing cache metadata", error))?;
    }
    Ok(())
}

fn replace_commit(connection: &Connection, commit: &Commit) -> Result<(), AppError> {
    let existing: Option<i64> = connection
        .query_row(
            "SELECT commit_id FROM commits WHERE oid = ?1",
            [&commit.oid],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| cache_error("looking up commit", error))?;
    if let Some(commit_id) = existing {
        connection
            .execute("DELETE FROM commit_paths WHERE commit_id = ?1", [commit_id])
            .map_err(|error| cache_error("removing old commit paths", error))?;
        connection
            .execute(
                "DELETE FROM commit_path_counts WHERE commit_id = ?1",
                [commit_id],
            )
            .map_err(|error| cache_error("removing old commit path count", error))?;
        connection
            .execute(
                "DELETE FROM hunks WHERE change_id IN (
                    SELECT change_id FROM changes WHERE commit_id = ?1
                )",
                [commit_id],
            )
            .map_err(|error| cache_error("removing old hunks", error))?;
        connection
            .execute("DELETE FROM changes WHERE commit_id = ?1", [commit_id])
            .map_err(|error| cache_error("removing old changes", error))?;
        connection
            .execute("DELETE FROM search_fts WHERE rowid = ?1", [commit_id])
            .map_err(|error| cache_error("removing old search document", error))?;
        connection
            .execute(
                "DELETE FROM commit_parents WHERE commit_id = ?1",
                [commit_id],
            )
            .map_err(|error| cache_error("removing old commit parents", error))?;
        let (message, length) = encode(&commit.message);
        connection
            .execute(
                "UPDATE commits SET message = ?2, message_length = ?3, commit_time = ?4 WHERE commit_id = ?1",
                params![commit_id, message, length, commit.time],
            )
            .map_err(|error| cache_error("updating commit", error))?;
    } else {
        insert_commit(connection, commit)?;
    }
    Ok(())
}

fn insert_commit(connection: &Connection, commit: &Commit) -> Result<(), AppError> {
    let (message, message_length) = encode(&commit.message);
    connection
        .execute(
            "INSERT INTO commits(position, oid, message, message_length, commit_time)
             VALUES ((SELECT COALESCE(MAX(position), -1) + 1 FROM commits), ?1, ?2, ?3, ?4)",
            params![commit.oid, message, message_length, commit.time],
        )
        .map_err(|error| cache_error("writing commit", error))?;
    Ok(())
}

fn insert_parents(connection: &Connection, commit: &Commit) -> Result<(), AppError> {
    let commit_id = commit_id(connection, &commit.oid)?;
    for (position, parent) in commit.parents.iter().enumerate() {
        let parent_id: Option<i64> = connection
            .query_row(
                "SELECT commit_id FROM commits WHERE oid = ?1",
                [parent],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| cache_error("looking up commit parent", error))?;
        connection
            .execute(
                "INSERT INTO commit_parents(commit_id, position, parent_id, external_oid)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    commit_id,
                    position as i64,
                    parent_id,
                    parent_id.is_none().then_some(parent)
                ],
            )
            .map_err(|error| cache_error("writing commit parent", error))?;
    }
    Ok(())
}

fn commit_id(connection: &Connection, oid: &str) -> Result<i64, AppError> {
    connection
        .query_row(
            "SELECT commit_id FROM commits WHERE oid = ?1",
            [oid],
            |row| row.get(0),
        )
        .map_err(|error| cache_error("looking up commit", error))
}

fn insert_document(
    connection: &Connection,
    commit: &Commit,
    paths: &[Vec<u8>],
) -> Result<(), AppError> {
    let (subject, body) = analysis::message_parts(&commit.message);
    let subject = analysis::searchable_text(&subject);
    let body = analysis::searchable_text(&body);
    let paths = paths
        .iter()
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    let paths = analysis::searchable_text(&paths);
    let commit_id = commit_id(connection, &commit.oid)?;
    connection
        .execute(
            "INSERT INTO search_fts(rowid, subject, body, paths) VALUES (?1, ?2, ?3, ?4)",
            params![commit_id, subject, body, paths],
        )
        .map_err(|error| cache_error("writing search document", error))?;
    Ok(())
}

fn insert_path_projection(
    connection: &rusqlite::Transaction<'_>,
    commit: &Commit,
    paths: &[ProjectedPath],
) -> Result<(), AppError> {
    let commit_id = commit_id(connection, &commit.oid)?;
    for path in paths {
        connection
            .execute(
                "INSERT INTO commit_paths(commit_id, path_key, path_basename, raw_path, path_order)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    commit_id,
                    path.key,
                    path.basename,
                    path.raw_path,
                    path.order,
                ],
            )
            .map_err(|error| cache_error("writing commit path projection", error))?;
    }
    connection
        .execute(
            "INSERT INTO commit_path_counts(commit_id, path_count) VALUES (?1, ?2)",
            params![commit_id, paths.len() as i64],
        )
        .map_err(|error| cache_error("writing commit path count", error))?;
    Ok(())
}

fn insert_changes(
    connection: &Connection,
    changes: &[Change],
) -> Result<HashMap<(String, i64), i64>, AppError> {
    let mut change_ids = HashMap::with_capacity(changes.len());
    for change in changes {
        let commit_id = commit_id(connection, &change.commit_oid)?;
        connection
            .execute(
                "INSERT INTO changes(commit_id, ordinal, status, old_path, new_path, old_blob, new_blob, old_mode, new_mode)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    commit_id,
                    change.ordinal,
                    change.status,
                    change.old_path,
                    change.new_path,
                    change.old_blob,
                    change.new_blob,
                    change.old_mode,
                    change.new_mode,
                ],
            )
            .map_err(|error| cache_error("writing change", error))?;
        let change_id = connection.last_insert_rowid();
        change_ids.insert((change.commit_oid.clone(), change.ordinal), change_id);
    }
    Ok(change_ids)
}

fn write_hunks(
    transaction: &rusqlite::Transaction<'_>,
    patches: &PatchStream,
    change_ids: &HashMap<(String, i64), i64>,
) -> Result<(), AppError> {
    if patches.is_empty() {
        return Ok(());
    }
    let mut writer = HunkWriter::new(transaction)?;
    patches.for_each_hunk(&mut |hunk| write_hunk(transaction, &mut writer, &hunk, change_ids))?;
    writer.finish(transaction)
}

fn write_hunk(
    transaction: &rusqlite::Transaction<'_>,
    writer: &mut HunkWriter,
    hunk: &Hunk,
    change_ids: &HashMap<(String, i64), i64>,
) -> Result<(), AppError> {
    let change_id = change_ids
        .get(&(hunk.commit_oid.clone(), hunk.change_ordinal))
        .copied()
        .ok_or_else(|| {
            cache_error(
                "writing hunk",
                format!(
                    "change {} ordinal {} is outside the current snapshot",
                    hunk.commit_oid, hunk.change_ordinal
                ),
            )
        })?;
    writer.write(transaction, change_id, hunk)
}

fn validate_connection(connection: &Connection, expected_commits: i64) -> Result<(), AppError> {
    let check: String = connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|error| cache_error("checking cache", error))?;
    if check != "ok" {
        return Err(AppError::operational(
            "error: validating cache transaction failed; retry",
        ));
    }
    let count = count_connection(connection, "SELECT COUNT(*) FROM commits")?;
    let fts = count_connection(connection, "SELECT COUNT(*) FROM search_fts_docsize")?;
    let fts_rows = count_connection(connection, "SELECT COUNT(*) FROM search_fts")?;
    let path_counts = count_connection(connection, "SELECT COUNT(*) FROM commit_path_counts")?;
    if count != expected_commits || fts != count || fts_rows != count || path_counts != count {
        return Err(AppError::operational(
            "error: validating cache transaction failed; retry",
        ));
    }
    Ok(())
}

fn count_connection(connection: &Connection, query: &str) -> Result<i64, AppError> {
    connection
        .query_row(query, [], |row| row.get(0))
        .map_err(|error| cache_error("counting cache rows", error))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use rusqlite::{Connection, params};

    use super::{HunkWriter, schema, write_hunk, write_hunks};
    use crate::{
        cache::read_hunks,
        git::{Hunk, PatchStream},
    };

    #[test]
    fn hunk_write_rejects_change_outside_current_snapshot() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(schema::SCHEMA).unwrap();
        connection
            .execute(
                "INSERT INTO commits(position, oid, message, message_length, commit_time)
                 VALUES (0, ?1, ?2, 0, 0)",
                params!["historical", Vec::<u8>::new()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO changes(change_id, commit_id, ordinal, status, old_mode, new_mode)
                 VALUES (1, 1, 0, 'M', '100644', '100644')",
                [],
            )
            .unwrap();

        let hunk = Hunk {
            commit_oid: "historical".to_owned(),
            change_ordinal: 0,
            ordinal: 0,
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
            text: b"@@ -1 +1 @@\n-old\n+new\n".to_vec(),
        };
        let transaction = connection.transaction().unwrap();
        let mut writer = HunkWriter::new(&transaction).unwrap();
        let error = write_hunk(&transaction, &mut writer, &hunk, &HashMap::new()).unwrap_err();

        assert!(error.to_string().contains("outside the current snapshot"));
    }

    #[test]
    fn hunk_writes_round_trip() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(schema::SCHEMA).unwrap();
        connection
            .execute(
                "INSERT INTO commits(position, oid, message, message_length, commit_time)
                 VALUES (0, ?1, ?2, 0, 0)",
                params!["current", Vec::<u8>::new()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO changes(change_id, commit_id, ordinal, status, old_mode, new_mode)
                 VALUES (1, 1, 0, 'M', '100644', '100644')",
                [],
            )
            .unwrap();
        let hunks = [Hunk {
            commit_oid: "current".to_owned(),
            change_ordinal: 0,
            ordinal: 0,
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
            text: b"@@ -1 +1 @@\n-old\n+new\n".to_vec(),
        }];
        let change_ids = HashMap::from([(("current".to_owned(), 0), 1)]);
        let transaction = connection.transaction().unwrap();
        let mut writer = HunkWriter::new(&transaction).unwrap();
        write_hunk(&transaction, &mut writer, &hunks[0], &change_ids).unwrap();
        writer.finish(&transaction).unwrap();
        transaction.commit().unwrap();
        let decoded = read_hunks(&connection, "current").unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].change_ordinal, hunks[0].change_ordinal);
        assert_eq!(decoded[0].old_start, hunks[0].old_start);
        assert_eq!(decoded[0].old_lines, hunks[0].old_lines);
        assert_eq!(decoded[0].new_start, hunks[0].new_start);
        assert_eq!(decoded[0].new_lines, hunks[0].new_lines);
        assert_eq!(decoded[0].text, hunks[0].text);
    }

    #[test]
    fn empty_hunks_skip_line_dictionary_setup() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(schema::SCHEMA).unwrap();
        // Dictionary loading would reject this malformed row if setup ran.
        connection
            .execute(
                "INSERT INTO hunk_line_blocks(block_id, first_line_id, line_count, text, text_length)
                 VALUES (1, 0, 1, ?1, 1)",
                params![Vec::<u8>::new()],
            )
            .unwrap();
        let transaction = connection.transaction().unwrap();
        write_hunks(
            &transaction,
            &PatchStream::empty_for_test(),
            &HashMap::new(),
        )
        .unwrap();
    }
}
