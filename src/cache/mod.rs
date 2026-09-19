mod schema;

use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    path::Path,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params, params_from_iter};

use crate::{
    analysis,
    app::AppError,
    git::{Change, Commit, Hunk, Snapshot},
};

const SCHEMA_VERSION: &str = "3";
const WAITING_MESSAGE: &str = "Waiting for another GitScry process...";

pub(crate) struct CacheState {
    pub(crate) default_ref: String,
    pub(crate) tip: String,
    pub(crate) object_format: String,
    pub(crate) shallow_boundaries: Vec<String>,
    pub(crate) missing_objects: Vec<String>,
    pub(crate) referenced_objects: Vec<String>,
    pub(crate) commits: Vec<String>,
    pub(crate) commit_count: usize,
}

pub(crate) enum Inspection {
    Missing,
    Damaged,
    Ready(CacheState),
}

pub(crate) struct SharedLock {
    file: File,
}

pub(crate) struct ExclusiveLock {
    file: File,
}

impl Drop for SharedLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

impl Drop for ExclusiveLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

pub(crate) fn acquire_shared(
    root: &Path,
    progress: &mut Vec<String>,
) -> Result<SharedLock, AppError> {
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

pub(crate) fn acquire_exclusive(
    root: &Path,
    progress: &mut Vec<String>,
) -> Result<ExclusiveLock, AppError> {
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

pub(crate) fn inspect(root: &Path) -> Inspection {
    let path = root.join(".gitscry/cache.sqlite");
    if !path.is_file() {
        return Inspection::Missing;
    }
    let Ok(connection) = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return Inspection::Damaged;
    };
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
        "search_documents",
        "search_fts",
        "commit_parents",
        "changes",
        "hunks",
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
    let document_count = count(connection, "SELECT COUNT(*) FROM search_documents")?;
    let fts_count = count(connection, "SELECT COUNT(*) FROM search_fts")?;
    let fts_document_count = count(connection, "SELECT COUNT(*) FROM search_fts_docsize")?;
    if commit_count != completed_count as i64
        || document_count != commit_count
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

pub(crate) fn shallow_warning(shallow_boundaries: &[String]) -> Option<String> {
    (!shallow_boundaries.is_empty())
        .then_some("warning: local history is shallow; cache material is incomplete.".to_owned())
}

pub(crate) fn missing_warning(missing_objects: &[String]) -> Option<String> {
    (!missing_objects.is_empty()).then_some(
        "warning: some local objects are missing; cache material is incomplete.".to_owned(),
    )
}

pub(crate) fn open(root: &Path) -> Result<Connection, AppError> {
    Connection::open_with_flags(
        root.join(".gitscry/cache.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|error| cache_error("opening cache", error))
}

pub(crate) fn preserve_damaged(root: &Path) -> Result<(), AppError> {
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
pub(crate) fn recover_previous(root: &Path) -> Result<(), AppError> {
    let directory = root.join(".gitscry");
    let final_path = directory.join("cache.sqlite");
    let previous = directory.join("cache.sqlite.previous");
    if !final_path.exists() && previous.exists() {
        fs::rename(previous, final_path)
            .map_err(|error| cache_error("recovering the previous cache", error))?;
    }
    Ok(())
}

pub(crate) fn publish(root: &Path, snapshot: &Snapshot) -> Result<(), AppError> {
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

pub(crate) fn append(root: &Path, snapshot: &Snapshot) -> Result<usize, AppError> {
    let path = root.join(".gitscry/cache.sqlite");
    let mut connection =
        Connection::open(&path).map_err(|error| cache_error("opening cache", error))?;
    connection
        .execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = FULL;")
        .map_err(|error| cache_error("configuring cache", error))?;
    let transaction = connection
        .transaction()
        .map_err(|error| cache_error("starting cache transaction", error))?;
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
    for commit in &snapshot.commits {
        replace_commit(&transaction, commit)?;
        insert_document(
            &transaction,
            commit,
            paths_by_commit
                .get(&commit.oid)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        )?;
    }
    for change in &snapshot.changes {
        insert_change(&transaction, change)?;
    }
    for hunk in &snapshot.hunks {
        insert_hunk(&transaction, hunk)?;
    }
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
        .execute("INSERT INTO search_fts(search_fts) VALUES ('rebuild')", [])
        .map_err(|error| cache_error("updating search index", error))?;
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
    for commit in &snapshot.commits {
        insert_commit(&transaction, commit)?;
        insert_document(
            &transaction,
            commit,
            paths_by_commit
                .get(&commit.oid)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        )?;
    }
    for change in &snapshot.changes {
        insert_change(&transaction, change)?;
    }
    for hunk in &snapshot.hunks {
        insert_hunk(&transaction, hunk)?;
    }
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
        .execute("INSERT INTO search_fts(search_fts) VALUES ('rebuild')", [])
        .map_err(|error| cache_error("building search index", error))?;
    transaction
        .commit()
        .map_err(|error| cache_error("committing staging cache", error))?;
    connection
        .close()
        .map_err(|(_, error)| cache_error("closing staging cache", error))
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
    connection
        .execute("DELETE FROM hunks WHERE commit_oid = ?1", [&commit.oid])
        .map_err(|error| cache_error("removing old hunks", error))?;
    connection
        .execute("DELETE FROM changes WHERE commit_oid = ?1", [&commit.oid])
        .map_err(|error| cache_error("removing old changes", error))?;
    connection
        .execute(
            "DELETE FROM search_documents WHERE commit_oid = ?1",
            [&commit.oid],
        )
        .map_err(|error| cache_error("removing old search document", error))?;
    connection
        .execute(
            "DELETE FROM commit_parents WHERE commit_oid = ?1",
            [&commit.oid],
        )
        .map_err(|error| cache_error("removing old commit parents", error))?;
    let updated = connection
        .execute(
            "UPDATE commits SET message = ?2, commit_time = ?3 WHERE oid = ?1",
            params![commit.oid, commit.message, commit.time],
        )
        .map_err(|error| cache_error("updating commit", error))?;
    if updated == 0 {
        connection
            .execute(
                "INSERT INTO commits(oid, message, commit_time) VALUES (?1, ?2, ?3)",
                params![commit.oid, commit.message, commit.time],
            )
            .map_err(|error| cache_error("writing commit", error))?;
    }
    insert_parents(connection, commit)
}

fn insert_commit(connection: &Connection, commit: &Commit) -> Result<(), AppError> {
    connection
        .execute(
            "INSERT INTO commits(oid, message, commit_time) VALUES (?1, ?2, ?3)",
            params![commit.oid, commit.message, commit.time],
        )
        .map_err(|error| cache_error("writing commit", error))?;
    insert_parents(connection, commit)
}

fn insert_parents(connection: &Connection, commit: &Commit) -> Result<(), AppError> {
    for (position, parent) in commit.parents.iter().enumerate() {
        connection
            .execute(
                "INSERT INTO commit_parents(commit_oid, position, parent_oid) VALUES (?1, ?2, ?3)",
                params![commit.oid, position as i64, parent],
            )
            .map_err(|error| cache_error("writing commit parent", error))?;
    }
    Ok(())
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
    connection
        .execute(
            "INSERT INTO search_documents(commit_oid, subject, body, paths) VALUES (?1, ?2, ?3, ?4)",
            params![commit.oid, subject, body, paths],
        )
        .map_err(|error| cache_error("writing search document", error))?;
    Ok(())
}

fn insert_change(connection: &Connection, change: &Change) -> Result<(), AppError> {
    connection
        .execute(
            "INSERT INTO changes VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                change.commit_oid,
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
        .map_err(|error| cache_error("writing first-parent change", error))?;
    Ok(())
}

fn insert_hunk(connection: &Connection, hunk: &Hunk) -> Result<(), AppError> {
    connection
        .execute(
            "INSERT INTO hunks VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                hunk.commit_oid,
                hunk.change_ordinal,
                hunk.ordinal,
                hunk.old_start,
                hunk.old_lines,
                hunk.new_start,
                hunk.new_lines,
                hunk.text,
            ],
        )
        .map_err(|error| cache_error("writing text hunk", error))?;
    Ok(())
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
    let documents = count_connection(connection, "SELECT COUNT(*) FROM search_documents")?;
    let fts = count_connection(connection, "SELECT COUNT(*) FROM search_fts_docsize")?;
    if count != expected_commits || documents != count || fts != count {
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

pub(crate) fn commits_for_objects(
    root: &Path,
    objects: &[String],
) -> Result<Vec<String>, AppError> {
    if objects.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", objects.len())
        .collect::<Vec<_>>()
        .join(", ");
    let query = format!(
        "SELECT DISTINCT commit_oid FROM changes
         WHERE old_blob IN ({placeholders}) OR new_blob IN ({placeholders})
         ORDER BY commit_oid"
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

pub(crate) fn boundary_refreshes(root: &Path) -> Result<Vec<String>, AppError> {
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
            "SELECT c.oid, p.parent_oid
             FROM commits AS c
             LEFT JOIN commit_parents AS p
               ON p.commit_oid = c.oid AND p.position = 0",
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
