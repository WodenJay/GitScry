use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use rusqlite::{Connection, OpenFlags};

use crate::app::AppError;

use super::{acquire_exclusive, cache_directory, cache_path};

pub(crate) struct ClearDataFile {
    pub(crate) name: String,
    pub(crate) size_bytes: u64,
    path: PathBuf,
}

#[derive(Default)]
pub(crate) struct ClearCounts {
    pub(crate) commits: u64,
    pub(crate) changes: u64,
    pub(crate) path_records: u64,
    pub(crate) hunks: u64,
    pub(crate) semantic_vectors: u64,
}

pub(crate) struct ClearReport {
    pub(crate) dry_run: bool,
    pub(crate) counts: Option<ClearCounts>,
    pub(crate) data_files: Vec<ClearDataFile>,
    pub(crate) before_bytes: u64,
    pub(crate) after_bytes: u64,
    pub(crate) released_bytes: u64,
}

pub(crate) fn clear(
    common_dir: &Path,
    dry_run: bool,
) -> Result<(Vec<String>, ClearReport), AppError> {
    let mut progress = Vec::new();
    let _lock = acquire_exclusive(common_dir, &mut progress)?;
    let directory = cache_directory(common_dir);
    let data_files = collect_data_files(&directory)?;
    let before_bytes = total_bytes(&data_files)?;
    let counts = read_counts(&cache_path(common_dir));

    if dry_run {
        return Ok((
            progress,
            ClearReport {
                dry_run,
                counts,
                data_files,
                before_bytes,
                after_bytes: 0,
                released_bytes: before_bytes,
            },
        ));
    }

    for data_file in &data_files {
        fs::remove_file(&data_file.path)
            .map_err(|error| clear_error("deleting cache data", error))?;
    }
    let after_bytes = total_bytes(&collect_data_files(&directory)?)?;
    let released_bytes = before_bytes.saturating_sub(after_bytes);

    Ok((
        progress,
        ClearReport {
            dry_run,
            counts,
            data_files,
            before_bytes,
            after_bytes,
            released_bytes,
        },
    ))
}

fn collect_data_files(directory: &Path) -> Result<Vec<ClearDataFile>, AppError> {
    let entries =
        fs::read_dir(directory).map_err(|error| clear_error("reading cache directory", error))?;
    let mut files = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|error| clear_error("reading cache directory entry", error))?;
        let name = entry.file_name();
        if name == OsStr::new("cache.lock") || name == OsStr::new(".gitignore") {
            continue;
        }
        let file_type = entry
            .file_type()
            .map_err(|error| clear_error("inspecting cache data", error))?;
        if !file_type.is_file() {
            return Err(clear_error(
                "inspecting cache data",
                format!("unsupported non-file entry {}", name.to_string_lossy()),
            ));
        }
        let size_bytes = entry
            .metadata()
            .map_err(|error| clear_error("measuring cache data", error))?
            .len();
        files.push(ClearDataFile {
            name: name.to_string_lossy().into_owned(),
            size_bytes,
            path: entry.path(),
        });
    }

    files.sort_by(|left, right| {
        match (left.name == "cache.sqlite", right.name == "cache.sqlite") {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => left.name.cmp(&right.name),
        }
    });
    Ok(files)
}

fn total_bytes(files: &[ClearDataFile]) -> Result<u64, AppError> {
    files.iter().try_fold(0_u64, |total, file| {
        total
            .checked_add(file.size_bytes)
            .ok_or_else(|| clear_error("summing cache data sizes", "byte count overflow"))
    })
}

fn read_counts(path: &Path) -> Option<ClearCounts> {
    if !path.is_file() {
        return Some(ClearCounts::default());
    }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    Some(ClearCounts {
        commits: table_count(&connection, "commits").ok()?,
        changes: table_count(&connection, "changes").ok()?,
        path_records: table_count(&connection, "commit_paths").ok()?,
        hunks: table_count(&connection, "hunks").ok()?,
        semantic_vectors: table_count(&connection, "semantic_vectors").ok()?,
    })
}

fn table_count(connection: &Connection, table: &str) -> rusqlite::Result<u64> {
    connection.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
}

fn clear_error(operation: &str, error: impl std::fmt::Display) -> AppError {
    AppError::operational(format!("error: {operation}: {error}; retry"))
}
