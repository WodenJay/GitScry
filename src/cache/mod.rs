//! Published query sessions and shared cache infrastructure.
//! Generation planning/publication lives in `generation`; row writes in `write`.

mod generation;
mod history;
mod payload;
mod query;
mod schema;
mod scope;
pub(crate) use scope::SearchFilter;
mod semantic;
mod write;
use crate::app::AppError;
pub(crate) use generation::prepare;
pub(crate) use history::HunkId;
pub(crate) use history::{CodeHunk, HistoryCommit, HistoryHunk, PatchHistoryHunk};
use payload::HunkReader;
pub(crate) use query::{RelationHistory, SearchCandidate, SearchMaterial};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
pub(crate) use semantic::{SemanticCandidate, SemanticPreference, maintain as maintain_semantic};
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};
const SCHEMA_VERSION: &str = "9";
const WAITING_MESSAGE: &str = "Waiting for another GitScry process...";

struct SharedLock {
    file: File,
}

struct ExclusiveLock {
    file: File,
}

impl Drop for SharedLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

pub(crate) struct QuerySession {
    root: PathBuf,
    connection: Connection,
    _lock: SharedLock,
    progress: Vec<String>,
    warnings: Vec<String>,
}

impl QuerySession {
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn progress(&self) -> &[String] {
        &self.progress
    }

    pub(crate) fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub(crate) fn completed_tip(&self) -> Result<String, AppError> {
        query_metadata(&self.connection, "completed_tip")?
            .ok_or_else(|| query_error("published cache has no completed tip"))
    }

    pub(crate) fn require_revision(&self, revision: &str) -> Result<(), AppError> {
        let present: i64 = self
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM commits WHERE oid = ?1)",
                [revision],
                |row| row.get(0),
            )
            .map_err(|error| query_error(format!("checking requested revision: {error}")))?;
        if present == 0 {
            return Err(query_error(format!(
                "revision {revision} is outside the published cache generation"
            )));
        }
        Ok(())
    }
    pub(crate) fn contains_revision(&self, revision: &str) -> Result<bool, AppError> {
        let present: i64 = self
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM commits WHERE oid = ?1)",
                [revision],
                |row| row.get(0),
            )
            .map_err(|error| query_error(format!("checking requested revision: {error}")))?;
        Ok(present != 0)
    }
}

impl Drop for ExclusiveLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn acquire_shared(root: &Path, progress: &mut Vec<String>) -> Result<SharedLock, AppError> {
    let file = open_lock_file(root)?;
    loop {
        match file.try_lock_shared() {
            Ok(()) => return Ok(SharedLock { file }),
            Err(std::fs::TryLockError::WouldBlock) => {
                report_wait(progress);
                thread::sleep(Duration::from_millis(25));
            }
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(lock_error("shared", error));
            }
        }
    }
}

fn acquire_exclusive(root: &Path, progress: &mut Vec<String>) -> Result<ExclusiveLock, AppError> {
    let file = open_lock_file(root)?;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(ExclusiveLock { file }),
            Err(std::fs::TryLockError::WouldBlock) => {
                report_wait(progress);
                thread::sleep(Duration::from_millis(25));
            }
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(lock_error("exclusive", error));
            }
        }
    }
}

fn open_lock_file(root: &Path) -> Result<File, AppError> {
    let directory = root.join(".gitscry");
    fs::create_dir_all(&directory).map_err(|error| cache_error("creating .gitscry", error))?;
    ensure_ignored(&directory)?;
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("cache.lock"))
        .map_err(|error| cache_error("opening cache lock", error))
}

fn report_wait(progress: &mut Vec<String>) {
    if !progress.iter().any(|line| line == WAITING_MESSAGE) {
        progress.push(WAITING_MESSAGE.to_owned());
    }
}

fn lock_error(mode: &str, error: impl std::fmt::Display) -> AppError {
    AppError::operational(format!(
        "error: acquiring {mode} cache lock: {error}; retry"
    ))
}

fn shallow_warning(has_boundaries: bool) -> Option<String> {
    has_boundaries
        .then_some("warning: local history is shallow; cache material is incomplete.".to_owned())
}
fn missing_warning(has_missing_objects: bool) -> Option<String> {
    has_missing_objects.then_some(
        "warning: some local objects are missing; cache material is incomplete.".to_owned(),
    )
}

pub(crate) fn open_query(root: &Path) -> Result<QuerySession, AppError> {
    if !root.join(".gitscry/cache.sqlite").is_file() {
        return Err(query_error(
            "no published cache found; run `gitscry index` first",
        ));
    }
    let mut progress = Vec::new();
    let lock = acquire_query_shared(root, &mut progress)?;
    let connection = Connection::open_with_flags(
        root.join(".gitscry/cache.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|error| query_error(format!("opening published cache: {error}")))?;
    validate_query_metadata(&connection)?;
    let warnings = query_warnings(&connection)?;
    Ok(QuerySession {
        root: root.to_owned(),
        connection,
        _lock: lock,
        progress,
        warnings,
    })
}

fn acquire_query_shared(root: &Path, progress: &mut Vec<String>) -> Result<SharedLock, AppError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join(".gitscry/cache.lock"))
        .map_err(|error| query_error(format!("opening published cache lock: {error}")))?;
    loop {
        match file.try_lock_shared() {
            Ok(()) => return Ok(SharedLock { file }),
            Err(std::fs::TryLockError::WouldBlock) => {
                report_wait(progress);
                thread::sleep(Duration::from_millis(25));
            }
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(lock_error("shared", error));
            }
        }
    }
}

fn validate_query_metadata(connection: &Connection) -> Result<(), AppError> {
    let schema_version = query_metadata(connection, "schema_version")?;
    if schema_version.as_deref() != Some(SCHEMA_VERSION) {
        return Err(query_error(
            "published cache schema is stale or unsupported; run `gitscry index`",
        ));
    }
    let completed_tip = query_metadata(connection, "completed_tip")?;
    let completed_count = query_metadata(connection, "completed_commit_count")?;
    let default_ref = query_metadata(connection, "default_ref")?;
    let object_format = query_metadata(connection, "object_format")?;
    let valid_tip = completed_tip.as_deref().is_some_and(valid_object_id);
    let valid_count = completed_count
        .as_deref()
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|count| count > 0);
    let valid_default_ref = default_ref
        .as_deref()
        .is_some_and(|value| !value.is_empty());
    let valid_object_format = matches!(object_format.as_deref(), Some("sha1" | "sha256"));
    if !valid_tip || !valid_count || !valid_default_ref || !valid_object_format {
        return Err(query_error(
            "published cache completion metadata is invalid",
        ));
    }
    Ok(())
}

fn query_warnings(connection: &Connection) -> Result<Vec<String>, AppError> {
    let (has_shallow, has_missing): (i64, i64) = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM shallow_boundaries), EXISTS(SELECT 1 FROM missing_objects)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| query_error(format!("reading cache completeness metadata: {error}")))?;
    let mut warnings = Vec::new();
    if let Some(warning) = shallow_warning(has_shallow != 0) {
        warnings.push(warning);
    }
    if let Some(warning) = missing_warning(has_missing != 0) {
        warnings.push(warning);
    }
    Ok(warnings)
}
fn query_metadata(connection: &Connection, key: &str) -> Result<Option<String>, AppError> {
    connection
        .query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|error| query_error(format!("reading published cache metadata: {error}")))
}

fn valid_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) fn decode_message(compressed: &[u8], length: i64) -> Result<Vec<u8>, AppError> {
    payload::decode(compressed, length, "commit message")
}

fn decode_message_row(
    row: &rusqlite::Row<'_>,
    compressed_index: usize,
    length_index: usize,
) -> Result<Vec<u8>, rusqlite::Error> {
    let compressed: Vec<u8> = row.get(compressed_index)?;
    let length: i64 = row.get(length_index)?;
    decode_message(&compressed, length).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            compressed_index,
            rusqlite::types::Type::Blob,
            Box::new(error),
        )
    })
}

pub(crate) fn message_parts(message: &[u8]) -> (String, String) {
    let message = String::from_utf8_lossy(message);
    let mut lines = message.splitn(2, '\n');
    let subject = lines
        .next()
        .unwrap_or_default()
        .trim_end_matches('\r')
        .to_owned();
    let body = lines.next().unwrap_or_default().to_owned();
    (subject, body)
}

pub(crate) struct DecodedHunk {
    pub(crate) change_ordinal: i64,
    pub(crate) hunk_ordinal: i64,
    pub(crate) old_start: i64,
    pub(crate) old_lines: i64,
    pub(crate) new_start: i64,
    pub(crate) new_lines: i64,
    pub(crate) text: Vec<u8>,
}

pub(crate) fn read_hunks(connection: &Connection, oid: &str) -> Result<Vec<DecodedHunk>, AppError> {
    let mut statement = connection
        .prepare(
            "SELECT ch.ordinal, h.old_start, h.old_lines, h.new_start, h.new_lines,
                    h.ordinal, h.payload_id
             FROM commits AS c
             JOIN changes AS ch ON ch.commit_id = c.commit_id
             JOIN hunks AS h ON h.change_id = ch.change_id
             WHERE c.oid = ?1
             ORDER BY ch.ordinal, h.ordinal",
        )
        .map_err(|error| cache_error("preparing encoded text hunks", error))?;
    let rows = statement
        .query_map([oid], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(6)?,
            ))
        })
        .map_err(|error| cache_error("reading encoded text hunks", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| cache_error("reading encoded text hunks", error))?;
    drop(statement);
    let mut reader = HunkReader::new(connection)?;
    rows.into_iter()
        .map(
            |(
                change_ordinal,
                hunk_ordinal,
                old_start,
                old_lines,
                new_start,
                new_lines,
                payload_id,
            )| {
                let text = reader.decode_payload(
                    payload_id,
                    &format!("{oid}/change {change_ordinal}/hunk {hunk_ordinal}"),
                )?;
                Ok(DecodedHunk {
                    change_ordinal,
                    hunk_ordinal,
                    old_start,
                    old_lines,
                    new_start,
                    new_lines,
                    text,
                })
            },
        )
        .collect()
}

fn ensure_ignored(directory: &Path) -> Result<(), AppError> {
    let path = directory.join(".gitignore");
    let mut contents = match fs::read(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(cache_error("reading .gitscry/.gitignore", error)),
    };
    if contents
        .split(|byte| *byte == b'\n')
        .any(|line| line == b"*")
    {
        return Ok(());
    }
    if !contents.is_empty() && !contents.ends_with(b"\n") {
        contents.push(b'\n');
    }
    contents.extend_from_slice(b"*\n");
    fs::write(path, contents).map_err(|error| cache_error("writing .gitscry/.gitignore", error))
}

fn cache_error(operation: &str, error: impl std::fmt::Display) -> AppError {
    AppError::operational(format!(
        "error: {operation}: {error}; delete .gitscry and retry"
    ))
}

fn query_error(reason: impl std::fmt::Display) -> AppError {
    AppError::operational(format!("error: {reason}; run `gitscry index` first"))
}
