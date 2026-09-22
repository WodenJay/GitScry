mod generation;
mod history;
mod payload;
mod query;
mod schema;
use crate::{
    analysis,
    app::AppError,
    git::{Change, Commit, Hunk, Snapshot},
};
pub(crate) use generation::prepare;
pub(crate) use history::{HistoryCommit, HistoryHunk};
use payload::{HunkReader, HunkWriter, encode};
pub(crate) use query::RelationHistory;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params, params_from_iter};
use std::collections::{HashMap, HashSet};
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
const SCHEMA_VERSION: &str = "6";
const WAITING_MESSAGE: &str = "Waiting for another GitScry process...";

struct CacheState {
    pub(crate) default_ref: String,
    pub(crate) tip: String,
    pub(crate) object_format: String,
    pub(crate) shallow_boundaries: Vec<String>,
    pub(crate) missing_objects: Vec<String>,
    pub(crate) referenced_objects: Vec<String>,
    pub(crate) commits: Vec<String>,
    pub(crate) commit_count: usize,
}

enum Inspection {
    Missing,
    Stale,
    Damaged,
    Ready(CacheState),
}

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
}

impl QuerySession {
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn progress(&self) -> &[String] {
        &self.progress
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

fn inspect(root: &Path) -> Inspection {
    let path = root.join(".gitscry/cache.sqlite");
    if !path.is_file() {
        return Inspection::Missing;
    }
    let Ok(connection) = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return Inspection::Damaged;
    };
    if metadata(&connection, "schema_version")
        .ok()
        .flatten()
        .is_some_and(|version| version != SCHEMA_VERSION)
    {
        return Inspection::Stale;
    }
    match inspect_connection(&connection) {
        Ok(state) => Inspection::Ready(state),
        Err(()) => Inspection::Damaged,
    }
}

fn inspect_path(path: &Path) -> Result<CacheState, AppError> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| cache_error("opening cache for validation", error))?;
    inspect_connection(&connection)
        .map_err(|()| AppError::operational("error: validating cache generation; retry"))
}

fn inspect_connection(connection: &Connection) -> Result<CacheState, ()> {
    let check: String = connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|_| ())?;
    if check != "ok" {
        return Err(());
    }

    for table in [
        "metadata",
        "shallow_boundaries",
        "missing_objects",
        "commits",
        "search_fts",
        "commit_parents",
        "changes",
        "hunks",
        "commit_paths",
        "commit_path_counts",
        "hunk_line_blocks",
        "hunk_token_blocks",
        "hunk_payloads",
    ] {
        let exists: Option<String> = connection
            .query_row(
                "SELECT name FROM sqlite_master WHERE name = ?1",
                [table],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| ())?;
        if exists.is_none() {
            return Err(());
        }
    }

    let version = metadata(connection, "schema_version")?.ok_or(())?;
    if version != SCHEMA_VERSION {
        return Err(());
    }
    let object_format = metadata(connection, "object_format")?.ok_or(())?;
    let default_ref = metadata(connection, "default_ref")?.ok_or(())?;
    let tip = metadata(connection, "completed_tip")?.ok_or(())?;
    let completed_count = metadata(connection, "completed_commit_count")?
        .ok_or(())?
        .parse::<usize>()
        .map_err(|_| ())?;

    let shallow_boundaries = string_rows(
        connection,
        "SELECT oid FROM shallow_boundaries ORDER BY oid",
    )?;
    let missing_objects = string_rows(connection, "SELECT oid FROM missing_objects ORDER BY oid")?;
    let commits = string_rows(connection, "SELECT oid FROM commits ORDER BY oid")?;
    let referenced_objects = string_rows(
        connection,
        "SELECT old_blob FROM changes WHERE old_blob IS NOT NULL
         UNION SELECT new_blob FROM changes WHERE new_blob IS NOT NULL
         ORDER BY 1",
    )?;

    let commit_count = count(connection, "SELECT COUNT(*) FROM commits")?;
    let fts_count = count(connection, "SELECT COUNT(*) FROM search_fts")?;
    let fts_document_count = count(connection, "SELECT COUNT(*) FROM search_fts_docsize")?;
    let path_count_rows = count(connection, "SELECT COUNT(*) FROM commit_path_counts")?;
    if commit_count != completed_count as i64
        || path_count_rows != commit_count
        || fts_count != commit_count
        || fts_document_count != commit_count
    {
        return Err(());
    }

    let commit_count = usize::try_from(commit_count).map_err(|_| ())?;
    if commits.len() != commit_count {
        return Err(());
    }
    Ok(CacheState {
        default_ref,
        tip,
        object_format,
        shallow_boundaries,
        missing_objects,
        referenced_objects,
        commits,
        commit_count,
    })
}

fn metadata(connection: &Connection, key: &str) -> Result<Option<String>, ()> {
    connection
        .query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|_| ())
}

fn string_rows(connection: &Connection, query: &str) -> Result<Vec<String>, ()> {
    let mut statement = connection.prepare(query).map_err(|_| ())?;
    statement
        .query_map([], |row| row.get(0))
        .map_err(|_| ())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ())
}

fn count(connection: &Connection, query: &str) -> Result<i64, ()> {
    connection
        .query_row(query, [], |row| row.get(0))
        .map_err(|_| ())
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

pub(crate) fn open_query() -> Result<QuerySession, AppError> {
    let cwd = std::env::current_dir()
        .map_err(|error| query_error(format!("reading current directory: {error}")))?;
    let root = find_query_root(&cwd)
        .ok_or_else(|| query_error("no published cache found; run `gitscry index` first"))?;
    let mut progress = Vec::new();
    let lock = acquire_query_shared(&root, &mut progress)?;
    let connection = Connection::open_with_flags(
        root.join(".gitscry/cache.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|error| query_error(format!("opening published cache: {error}")))?;
    validate_query_metadata(&connection)?;
    progress.extend(query_progress(&connection)?);
    Ok(QuerySession {
        root,
        connection,
        _lock: lock,
        progress,
    })
}

fn find_query_root(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_owned();
    loop {
        if current.join(".gitscry/cache.sqlite").is_file() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
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

fn query_progress(connection: &Connection) -> Result<Vec<String>, AppError> {
    let (has_shallow, has_missing): (i64, i64) = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM shallow_boundaries), EXISTS(SELECT 1 FROM missing_objects)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| query_error(format!("reading cache completeness metadata: {error}")))?;
    let mut progress = Vec::new();
    if let Some(warning) = shallow_warning(has_shallow != 0) {
        progress.push(warning);
    }
    if let Some(warning) = missing_warning(has_missing != 0) {
        progress.push(warning);
    }
    Ok(progress)
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

fn preserve_damaged(root: &Path) -> Result<(), AppError> {
    let directory = root.join(".gitscry");
    let path = directory.join("cache.sqlite");
    if !path.exists() {
        return Ok(());
    }
    let preserved = directory.join(format!(
        "cache.sqlite.corrupt-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::copy(path, preserved).map_err(|error| cache_error("preserving damaged cache", error))?;
    Ok(())
}
fn recover_previous(root: &Path) -> Result<(), AppError> {
    let directory = root.join(".gitscry");
    let final_path = directory.join("cache.sqlite");
    let previous = directory.join("cache.sqlite.previous");
    if !final_path.exists() && previous.exists() {
        fs::rename(previous, final_path)
            .map_err(|error| cache_error("recovering the previous cache", error))?;
    }
    Ok(())
}

fn publish(root: &Path, snapshot: &Snapshot) -> Result<(), AppError> {
    let directory = root.join(".gitscry");
    fs::create_dir_all(&directory).map_err(|error| cache_error("creating .gitscry", error))?;
    ensure_ignored(&directory)?;

    let final_path = directory.join("cache.sqlite");
    let staging = directory.join(format!("cache.sqlite.staging-{}", std::process::id()));
    let _ = fs::remove_file(&staging);
    if let Err(error) = build(&staging, snapshot) {
        let _ = fs::remove_file(&staging);
        return Err(error);
    }
    if let Err(error) = validate(&staging, snapshot.commits.len()) {
        let _ = fs::remove_file(&staging);
        return Err(error);
    }

    atomic_replace(&directory, &final_path, &staging)
}

fn append(root: &Path, snapshot: &Snapshot) -> Result<usize, AppError> {
    let path = root.join(".gitscry/cache.sqlite");
    let mut connection =
        Connection::open(&path).map_err(|error| cache_error("opening cache", error))?;
    connection
        .execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = FULL;")
        .map_err(|error| cache_error("configuring cache", error))?;
    let transaction = connection
        .transaction()
        .map_err(|error| cache_error("starting cache transaction", error))?;
    let paths_by_commit = paths_by_commit(snapshot);
    let projected_paths = projected_paths_by_commit(snapshot);
    for commit in &snapshot.commits {
        replace_commit(&transaction, commit)?;
    }
    for commit in &snapshot.commits {
        insert_parents(&transaction, commit)?;
        insert_document(
            &transaction,
            commit,
            paths_by_commit
                .get(&commit.oid)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        )?;
    }
    let change_ids = insert_changes(&transaction, &snapshot.changes)?;
    for commit in &snapshot.commits {
        insert_path_projection(
            &transaction,
            commit,
            projected_paths
                .get(&commit.oid)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        )?;
    }
    write_hunks(&transaction, &snapshot.hunks, &change_ids)?;
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

fn build(path: &Path, snapshot: &Snapshot) -> Result<(), AppError> {
    let mut connection =
        Connection::open(path).map_err(|error| cache_error("opening staging cache", error))?;
    connection
        .execute_batch(schema::SCHEMA)
        .map_err(|error| cache_error("creating cache schema", error))?;
    let transaction = connection
        .transaction()
        .map_err(|error| cache_error("starting cache transaction", error))?;
    replace_metadata(&transaction, snapshot)?;
    let paths_by_commit = paths_by_commit(snapshot);
    let projected_paths = projected_paths_by_commit(snapshot);
    for commit in &snapshot.commits {
        insert_commit(&transaction, commit)?;
    }
    for commit in &snapshot.commits {
        insert_parents(&transaction, commit)?;
        insert_document(
            &transaction,
            commit,
            paths_by_commit
                .get(&commit.oid)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        )?;
    }
    let change_ids = insert_changes(&transaction, &snapshot.changes)?;
    for commit in &snapshot.commits {
        insert_path_projection(
            &transaction,
            commit,
            projected_paths
                .get(&commit.oid)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        )?;
    }
    write_hunks(&transaction, &snapshot.hunks, &change_ids)?;
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
    hunks: &[Hunk],
    change_ids: &HashMap<(String, i64), i64>,
) -> Result<(), AppError> {
    let mut writer = HunkWriter::new(transaction)?;
    for hunk in hunks {
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
        writer.write(transaction, change_id, hunk)?;
    }
    writer.finish(transaction)
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

fn validate(path: &Path, expected_commits: usize) -> Result<(), AppError> {
    let state = inspect_path(path)?;
    if state.commit_count != expected_commits {
        return Err(AppError::operational(
            "error: validating staging cache failed; retry",
        ));
    }
    Ok(())
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

#[cfg(windows)]
fn atomic_replace(directory: &Path, final_path: &Path, staging: &Path) -> Result<(), AppError> {
    if final_path.exists() {
        let previous = directory.join("cache.sqlite.previous");
        let _ = fs::remove_file(&previous);
        fs::rename(final_path, &previous)
            .map_err(|error| cache_error("preserving the previous cache", error))?;
        if let Err(error) = fs::rename(staging, final_path) {
            let _ = fs::rename(&previous, final_path);
            let _ = fs::remove_file(staging);
            return Err(cache_error("publishing the cache", error));
        }
        let _ = fs::remove_file(previous);
    } else {
        fs::rename(staging, final_path)
            .map_err(|error| cache_error("publishing the cache", error))?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn atomic_replace(_directory: &Path, final_path: &Path, staging: &Path) -> Result<(), AppError> {
    fs::rename(staging, final_path).map_err(|error| cache_error("publishing the cache", error))
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

fn commits_for_objects(root: &Path, objects: &[String]) -> Result<Vec<String>, AppError> {
    if objects.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", objects.len())
        .collect::<Vec<_>>()
        .join(", ");
    let query = format!(
        "SELECT DISTINCT c.oid
         FROM changes AS ch
         JOIN commits AS c ON c.commit_id = ch.commit_id
         WHERE ch.old_blob IN ({placeholders}) OR ch.new_blob IN ({placeholders})
         ORDER BY c.oid",
    );
    let values = objects.iter().chain(objects.iter());
    let connection = Connection::open_with_flags(
        root.join(".gitscry/cache.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|error| cache_error("opening cache", error))?;
    let mut statement = connection
        .prepare(&query)
        .map_err(|error| cache_error("preparing missing-object lookup", error))?;
    statement
        .query_map(params_from_iter(values), |row| row.get(0))
        .map_err(|error| cache_error("reading missing-object lookup", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| cache_error("reading missing-object lookup", error))
}

fn boundary_refreshes(root: &Path) -> Result<Vec<String>, AppError> {
    let connection = Connection::open_with_flags(
        root.join(".gitscry/cache.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|error| cache_error("opening cache", error))?;
    let cached = string_rows(&connection, "SELECT oid FROM commits")
        .map_err(|()| AppError::operational("error: reading cached commits; retry"))?
        .into_iter()
        .collect::<HashSet<_>>();
    let mut statement = connection
        .prepare(
            "SELECT c.oid, COALESCE(parent.oid, p.external_oid)
             FROM commits AS c
             LEFT JOIN commit_parents AS p
               ON p.commit_id = c.commit_id AND p.position = 0
             LEFT JOIN commits AS parent ON parent.commit_id = p.parent_id",
        )
        .map_err(|error| cache_error("preparing boundary lookup", error))?;
    let mut refresh = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .map_err(|error| cache_error("reading boundary lookup", error))?
        .filter_map(|row| match row {
            Ok((commit, Some(parent))) if !cached.contains(&parent) => Some(Ok(commit)),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| cache_error("reading boundary lookup", error))?;
    refresh.sort();
    refresh.dedup();
    Ok(refresh)
}

fn cache_error(operation: &str, error: impl std::fmt::Display) -> AppError {
    AppError::operational(format!(
        "error: {operation}: {error}; delete .gitscry and retry"
    ))
}

fn query_error(reason: impl std::fmt::Display) -> AppError {
    AppError::operational(format!("error: {reason}; run `gitscry index` first"))
}
