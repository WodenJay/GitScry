mod schema;

use std::{fs, path::Path};

use rusqlite::{Connection, params};

use crate::{
    app::AppError,
    git::{Change, Commit, Hunk, Snapshot},
};

const SCHEMA_VERSION: &str = "1";

pub(crate) fn publish(root: &Path, snapshot: &Snapshot) -> Result<(), AppError> {
    let directory = root.join(".gitscry");
    fs::create_dir_all(&directory).map_err(|error| cache_error("creating .gitscry", error))?;
    ensure_ignored(&directory)?;

    let final_path = directory.join("cache.sqlite");
    if is_current(&final_path, snapshot)? {
        return Ok(());
    }

    let staging = directory.join(format!("cache.sqlite.staging-{}", std::process::id()));
    let _ = fs::remove_file(&staging);
    let result =
        build(&staging, snapshot).and_then(|()| validate(&staging, snapshot.commits.len()));
    if let Err(error) = result {
        let _ = fs::remove_file(&staging);
        return Err(error);
    }

    if final_path.exists() {
        let previous = directory.join("cache.sqlite.previous");
        let _ = fs::remove_file(&previous);
        fs::rename(&final_path, &previous)
            .map_err(|error| cache_error("preserving the previous cache", error))?;
        if let Err(error) = fs::rename(&staging, &final_path) {
            let _ = fs::rename(&previous, &final_path);
            let _ = fs::remove_file(&staging);
            return Err(cache_error("publishing the cache", error));
        }
        let _ = fs::remove_file(previous);
    } else {
        fs::rename(&staging, &final_path)
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

fn is_current(path: &Path, snapshot: &Snapshot) -> Result<bool, AppError> {
    if !path.exists() {
        return Ok(false);
    }
    let connection = match Connection::open(path) {
        Ok(connection) => connection,
        Err(_) => return Ok(false),
    };
    let query = |key: &str| {
        connection.query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| {
            row.get::<_, String>(0)
        })
    };
    Ok(
        query("schema_version").ok().as_deref() == Some(SCHEMA_VERSION)
            && query("completed_tip").ok().as_deref() == Some(snapshot.tip.as_str())
            && query("default_ref").ok().as_deref() == Some(snapshot.default_ref.as_str())
            && query("object_format").ok().as_deref() == Some(snapshot.object_format.as_str()),
    )
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

    for (key, value) in [
        ("schema_version", SCHEMA_VERSION),
        ("object_format", snapshot.object_format.as_str()),
        ("default_ref", snapshot.default_ref.as_str()),
        ("completed_tip", snapshot.tip.as_str()),
    ] {
        transaction
            .execute(
                "INSERT INTO metadata(key, value) VALUES (?1, ?2)",
                [key, value],
            )
            .map_err(|error| cache_error("writing cache metadata", error))?;
    }
    transaction
        .execute(
            "INSERT INTO metadata(key, value) VALUES ('completed_commit_count', ?1)",
            [snapshot.commits.len().to_string()],
        )
        .map_err(|error| cache_error("writing completed commit count", error))?;

    for oid in &snapshot.shallow_boundaries {
        transaction
            .execute("INSERT INTO shallow_boundaries(oid) VALUES (?1)", [oid])
            .map_err(|error| cache_error("writing shallow boundary", error))?;
    }
    for commit in &snapshot.commits {
        insert_commit(&transaction, commit)?;
    }
    for change in &snapshot.changes {
        insert_change(&transaction, change)?;
    }
    for hunk in &snapshot.hunks {
        insert_hunk(&transaction, hunk)?;
    }
    transaction
        .commit()
        .map_err(|error| cache_error("committing staging cache", error))?;
    connection
        .close()
        .map_err(|(_, error)| cache_error("closing staging cache", error))
}

fn insert_commit(connection: &Connection, commit: &Commit) -> Result<(), AppError> {
    connection
        .execute(
            "INSERT INTO commits(oid, message, commit_time) VALUES (?1, ?2, ?3)",
            params![commit.oid, commit.message, commit.time],
        )
        .map_err(|error| cache_error("writing commit", error))?;
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
    let connection =
        Connection::open(path).map_err(|error| cache_error("validating staging cache", error))?;
    let check: String = connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|error| cache_error("checking staging cache", error))?;
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM commits", [], |row| row.get(0))
        .map_err(|error| cache_error("counting staged commits", error))?;
    if check != "ok" || count != expected_commits as i64 {
        return Err(AppError::operational(
            "error: validating staging cache failed; delete .gitscry and retry",
        ));
    }
    Ok(())
}

fn cache_error(operation: &str, error: impl std::fmt::Display) -> AppError {
    AppError::operational(format!(
        "error: {operation}: {error}; delete .gitscry and retry"
    ))
}
