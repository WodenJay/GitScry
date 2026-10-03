use std::{fs, path::Path};

use rusqlite::{Connection, OpenFlags};
use tempfile::TempPath;

use crate::{
    app::AppError,
    git::{Hunk, Repository},
};

use super::{QueryLock, acquire_exclusive, cache_error, cache_path};

#[derive(Default)]
pub(crate) struct PruneCounts {
    pub(crate) commits: u64,
    pub(crate) changes: u64,
    pub(crate) path_records: u64,
    pub(crate) hunks: u64,
    pub(crate) semantic_vectors: u64,
}

pub(crate) struct PruneReport {
    pub(crate) dry_run: bool,
    pub(crate) counts: PruneCounts,
    pub(crate) before_bytes: u64,
    pub(crate) after_bytes: Option<u64>,
    pub(crate) released_bytes: Option<u64>,
}

pub(crate) fn prune(
    repository: &Repository,
    dry_run: bool,
) -> Result<(Vec<String>, PruneReport), AppError> {
    let mut progress = Vec::new();
    let lock = acquire_exclusive(&repository.common_dir, &mut progress, false)?;
    let _lock = QueryLock::Exclusive { _guard: lock };
    let directory = super::cache_directory(&repository.common_dir);
    let data_files = super::clear::collect_data_files(&directory)?;
    let before_bytes = super::clear::total_bytes(&data_files)?;
    let path = cache_path(&repository.common_dir);

    if !path.is_file() {
        return Ok((
            progress,
            PruneReport {
                dry_run,
                counts: PruneCounts::default(),
                before_bytes,
                after_bytes: (!dry_run).then_some(before_bytes),
                released_bytes: (!dry_run).then_some(0),
            },
        ));
    }

    let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| cache_error("opening cache for prune", error))?;
    let object_ids = cached_commit_oids(&connection)?;
    super::generation::validate(&path, object_ids.len())?;
    let candidates = repository.missing_objects(&object_ids)?;
    install_candidates(&connection, &candidates)?;
    let counts = count_candidates(&connection)?;

    if dry_run {
        return Ok((
            progress,
            PruneReport {
                dry_run,
                counts,
                before_bytes,
                after_bytes: None,
                released_bytes: None,
            },
        ));
    }
    if candidates.is_empty() {
        return Ok((
            progress,
            PruneReport {
                dry_run,
                counts,
                before_bytes,
                after_bytes: Some(before_bytes),
                released_bytes: Some(0),
            },
        ));
    }
    drop(connection);

    let staging = temporary_database(&directory, "cache-prune-")?;
    fs::copy(&path, &staging).map_err(|error| cache_error("copying cache for prune", error))?;
    let archive = temporary_database(&directory, "cache-hunks-")?;
    let mut staging_connection = Connection::open(&staging)
        .map_err(|error| cache_error("opening staged cache for prune", error))?;
    staging_connection
        .execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = FULL;")
        .map_err(|error| cache_error("configuring staged cache for prune", error))?;
    install_candidates(&staging_connection, &candidates)?;
    build_hunk_archive(&staging_connection, &archive)?;
    let remaining_commits = apply_prune(&mut staging_connection, &archive)?;
    staging_connection
        .execute_batch("VACUUM")
        .map_err(|error| cache_error("compacting pruned cache", error))?;
    staging_connection
        .close()
        .map_err(|(_, error)| cache_error("closing staged cache after prune", error))?;
    super::generation::validate(&staging, remaining_commits)?;

    let still_missing = repository.missing_objects(&candidates)?;
    if still_missing != candidates {
        return Err(AppError::operational(
            "error: Git object availability changed during prune; retry",
        ));
    }

    super::generation::atomic_replace(&directory, &path, &staging)?;
    drop(archive);
    drop(staging);
    let after_bytes = super::clear::total_bytes(&super::clear::collect_data_files(&directory)?)?;
    let released_bytes = before_bytes.saturating_sub(after_bytes);
    Ok((
        progress,
        PruneReport {
            dry_run,
            counts,
            before_bytes,
            after_bytes: Some(after_bytes),
            released_bytes: Some(released_bytes),
        },
    ))
}

fn cached_commit_oids(connection: &Connection) -> Result<Vec<String>, AppError> {
    let mut statement = connection
        .prepare("SELECT oid FROM commits ORDER BY position")
        .map_err(|error| cache_error("preparing cached commit IDs for prune", error))?;
    statement
        .query_map([], |row| row.get(0))
        .map_err(|error| cache_error("reading cached commit IDs for prune", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| cache_error("reading cached commit IDs for prune", error))
}

fn install_candidates(connection: &Connection, candidates: &[String]) -> Result<(), AppError> {
    connection
        .execute_batch("CREATE TEMP TABLE prune_candidates (oid TEXT PRIMARY KEY) STRICT;")
        .map_err(|error| cache_error("creating prune candidate list", error))?;
    let mut statement = connection
        .prepare("INSERT INTO temp.prune_candidates(oid) VALUES (?1)")
        .map_err(|error| cache_error("preparing prune candidate list", error))?;
    for oid in candidates {
        statement
            .execute([oid])
            .map_err(|error| cache_error("recording prune candidate", error))?;
    }
    Ok(())
}

fn count_candidates(connection: &Connection) -> Result<PruneCounts, AppError> {
    Ok(PruneCounts {
        commits: count(
            connection,
            "SELECT COUNT(*) FROM commits c JOIN temp.prune_candidates p ON p.oid = c.oid",
            "counting prune candidates",
        )?,
        changes: count(
            connection,
            "SELECT COUNT(*) FROM changes ch JOIN commits c ON c.commit_id = ch.commit_id
             JOIN temp.prune_candidates p ON p.oid = c.oid",
            "counting pruned changes",
        )?,
        path_records: count(
            connection,
            "SELECT COUNT(*) FROM commit_paths cp JOIN commits c ON c.commit_id = cp.commit_id
             JOIN temp.prune_candidates p ON p.oid = c.oid",
            "counting pruned path records",
        )?,
        hunks: count(
            connection,
            "SELECT COUNT(*) FROM hunks h JOIN changes ch ON ch.change_id = h.change_id
             JOIN commits c ON c.commit_id = ch.commit_id
             JOIN temp.prune_candidates p ON p.oid = c.oid",
            "counting pruned hunks",
        )?,
        semantic_vectors: count(
            connection,
            "SELECT COUNT(*) FROM semantic_vectors v JOIN commits c ON c.commit_id = v.commit_id
             JOIN temp.prune_candidates p ON p.oid = c.oid",
            "counting pruned semantic vectors",
        )?,
    })
}

fn count(connection: &Connection, query: &str, operation: &str) -> Result<u64, AppError> {
    let count: i64 = connection
        .query_row(query, [], |row| row.get(0))
        .map_err(|error| cache_error(operation, error))?;
    u64::try_from(count).map_err(|_| cache_error(operation, "invalid row count"))
}

fn temporary_database(directory: &Path, prefix: &str) -> Result<TempPath, AppError> {
    tempfile::Builder::new()
        .prefix(prefix)
        .suffix(".sqlite")
        .tempfile_in(directory)
        .map(|file| file.into_temp_path())
        .map_err(|error| cache_error("creating temporary cache database", error))
}

fn build_hunk_archive(source: &Connection, archive_path: &Path) -> Result<(), AppError> {
    let mut archive = Connection::open(archive_path)
        .map_err(|error| cache_error("opening temporary hunk archive", error))?;
    archive
        .execute_batch(super::schema::SCHEMA)
        .map_err(|error| cache_error("creating temporary hunk archive", error))?;
    archive
        .execute_batch("PRAGMA foreign_keys = OFF; PRAGMA synchronous = FULL;")
        .map_err(|error| cache_error("configuring temporary hunk archive", error))?;
    let transaction = archive
        .transaction()
        .map_err(|error| cache_error("starting temporary hunk archive", error))?;
    let mut writer = super::payload::HunkWriter::new(&transaction)?;
    let mut reader = super::payload::HunkReader::new(source)?;
    let mut statement = source
        .prepare(
            "SELECT c.oid, ch.change_id, ch.ordinal, h.ordinal, h.old_start,
                    h.old_lines, h.new_start, h.new_lines, h.payload_id
             FROM hunks h
             JOIN changes ch ON ch.change_id = h.change_id
             JOIN commits c ON c.commit_id = ch.commit_id
             WHERE NOT EXISTS (
                 SELECT 1 FROM temp.prune_candidates p WHERE p.oid = c.oid
             )
             ORDER BY c.position, ch.ordinal, h.ordinal",
        )
        .map_err(|error| cache_error("preparing surviving hunk archive", error))?;
    let mut rows = statement
        .query([])
        .map_err(|error| cache_error("reading surviving hunk archive", error))?;
    while let Some(row) = rows
        .next()
        .map_err(|error| cache_error("reading surviving hunk archive", error))?
    {
        let commit_oid: String = row
            .get(0)
            .map_err(|error| cache_error("reading surviving hunk archive", error))?;
        let change_id: i64 = row
            .get(1)
            .map_err(|error| cache_error("reading surviving hunk archive", error))?;
        let change_ordinal: i64 = row
            .get(2)
            .map_err(|error| cache_error("reading surviving hunk archive", error))?;
        let ordinal: i64 = row
            .get(3)
            .map_err(|error| cache_error("reading surviving hunk archive", error))?;
        let old_start: i64 = row
            .get(4)
            .map_err(|error| cache_error("reading surviving hunk archive", error))?;
        let old_lines: i64 = row
            .get(5)
            .map_err(|error| cache_error("reading surviving hunk archive", error))?;
        let new_start: i64 = row
            .get(6)
            .map_err(|error| cache_error("reading surviving hunk archive", error))?;
        let new_lines: i64 = row
            .get(7)
            .map_err(|error| cache_error("reading surviving hunk archive", error))?;
        let payload_id: i64 = row
            .get(8)
            .map_err(|error| cache_error("reading surviving hunk archive", error))?;
        let material = format!("{commit_oid}/change {change_ordinal}/hunk {ordinal}");
        let text = reader.decode_payload(payload_id, &material)?;
        reader.clear_decoded_blocks();
        writer.write(
            &transaction,
            change_id,
            &Hunk {
                commit_oid,
                change_ordinal,
                ordinal,
                old_start,
                old_lines,
                new_start,
                new_lines,
                text,
            },
        )?;
    }
    drop(rows);
    drop(statement);
    drop(reader);
    writer.finish(&transaction)?;
    transaction
        .commit()
        .map_err(|error| cache_error("finishing temporary hunk archive", error))?;
    archive
        .close()
        .map_err(|(_, error)| cache_error("closing temporary hunk archive", error))
}

fn apply_prune(connection: &mut Connection, archive_path: &Path) -> Result<usize, AppError> {
    let archive_path = archive_path.to_string_lossy();
    connection
        .execute(
            "ATTACH DATABASE ?1 AS prune_archive",
            [archive_path.as_ref()],
        )
        .map_err(|error| cache_error("attaching compact hunk archive", error))?;
    let transaction = connection
        .transaction()
        .map_err(|error| cache_error("starting cache prune transaction", error))?;
    transaction
        .execute_batch(
            "DELETE FROM hunks;
             DELETE FROM hunk_payloads;
             DELETE FROM hunk_token_blocks;
             DELETE FROM hunk_line_blocks;
             UPDATE commit_parents
             SET external_oid = (SELECT oid FROM commits WHERE commit_id = commit_parents.parent_id),
                 parent_id = NULL
             WHERE parent_id IN (
                 SELECT c.commit_id FROM commits c
                 JOIN temp.prune_candidates p ON p.oid = c.oid
             );
             DELETE FROM commit_parents WHERE commit_id IN (
                 SELECT c.commit_id FROM commits c
                 JOIN temp.prune_candidates p ON p.oid = c.oid
             );
             DELETE FROM search_fts WHERE rowid IN (
                 SELECT c.commit_id FROM commits c
                 JOIN temp.prune_candidates p ON p.oid = c.oid
             );
             DELETE FROM commit_paths WHERE commit_id IN (
                 SELECT c.commit_id FROM commits c
                 JOIN temp.prune_candidates p ON p.oid = c.oid
             );
             DELETE FROM commit_path_counts WHERE commit_id IN (
                 SELECT c.commit_id FROM commits c
                 JOIN temp.prune_candidates p ON p.oid = c.oid
             );
             DELETE FROM changes WHERE commit_id IN (
                 SELECT c.commit_id FROM commits c
                 JOIN temp.prune_candidates p ON p.oid = c.oid
             );
             DELETE FROM shallow_boundaries WHERE oid IN (SELECT oid FROM temp.prune_candidates);
             DELETE FROM missing_objects WHERE oid NOT IN (
                 SELECT old_blob FROM changes WHERE old_blob IS NOT NULL
                 UNION SELECT new_blob FROM changes WHERE new_blob IS NOT NULL
             );
             DELETE FROM commits WHERE oid IN (SELECT oid FROM temp.prune_candidates);
             INSERT INTO hunk_line_blocks SELECT * FROM prune_archive.hunk_line_blocks;
             INSERT INTO hunk_token_blocks SELECT * FROM prune_archive.hunk_token_blocks;
             INSERT INTO hunk_payloads SELECT * FROM prune_archive.hunk_payloads;
             INSERT INTO hunks SELECT * FROM prune_archive.hunks;
             INSERT INTO search_fts(search_fts) VALUES ('optimize');",
        )
        .map_err(|error| cache_error("deleting pruned cache material", error))?;

    let remaining_commits = count(
        &transaction,
        "SELECT COUNT(*) FROM commits",
        "counting remaining cached commits",
    )?;
    let remaining_commits = usize::try_from(remaining_commits).map_err(|_| {
        cache_error(
            "counting remaining cached commits",
            "count exceeded platform limits",
        )
    })?;
    transaction
        .execute(
            "UPDATE metadata SET value = ?1 WHERE key = 'completed_commit_count'",
            [remaining_commits.to_string()],
        )
        .map_err(|error| cache_error("updating pruned commit count", error))?;
    transaction
        .execute(
            "UPDATE metadata
             SET value = COALESCE((SELECT oid FROM commits ORDER BY position DESC LIMIT 1), value)
             WHERE key = 'completed_tip'
               AND NOT EXISTS (SELECT 1 FROM commits WHERE oid = value)",
            [],
        )
        .map_err(|error| cache_error("updating pruned cache tip", error))?;
    transaction
        .commit()
        .map_err(|error| cache_error("committing cache prune", error))?;
    connection
        .execute_batch("DETACH DATABASE prune_archive")
        .map_err(|error| cache_error("detaching compact hunk archive", error))?;
    Ok(remaining_commits)
}
