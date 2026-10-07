//! Rebuildable patch fingerprints, independent of the published history generation.
use crate::app::AppError;
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    fs::{File, OpenOptions},
    path::Path,
};

pub(crate) struct PatchFingerprints {
    connection: Connection,
    _lock: File,
}

pub(crate) struct PatchFingerprint {
    pub(crate) integrity: String,
    pub(crate) fingerprint: Option<Vec<u8>>,
}

impl PatchFingerprints {
    pub(crate) fn open(common_dir: &Path, version: i64) -> Result<Self, AppError> {
        let directory = super::cache_directory(common_dir);
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("patch-relationships.lock"))
            .map_err(cache_error)?;
        lock.lock().map_err(cache_error)?;
        let connection =
            Connection::open(directory.join("patch-relationships.sqlite")).map_err(cache_error)?;
        connection.execute_batch("CREATE TABLE IF NOT EXISTS fingerprints (oid TEXT PRIMARY KEY, integrity TEXT NOT NULL, fingerprint BLOB)").map_err(cache_error)?;
        let stored: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(cache_error)?;
        if stored != version {
            connection
                .execute_batch(&format!(
                    "BEGIN; DELETE FROM fingerprints; PRAGMA user_version={version}; COMMIT;"
                ))
                .map_err(cache_error)?;
        }
        Ok(Self {
            connection,
            _lock: lock,
        })
    }

    pub(crate) fn get(&self, oid: &str) -> Result<Option<PatchFingerprint>, AppError> {
        self.connection
            .query_row(
                "SELECT integrity, fingerprint FROM fingerprints WHERE oid=?1",
                [oid],
                |row| {
                    Ok(PatchFingerprint {
                        integrity: row.get(0)?,
                        fingerprint: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(cache_error)
    }

    pub(crate) fn put(
        &self,
        oid: &str,
        integrity: &str,
        fingerprint: Option<&[u8]>,
    ) -> Result<(), AppError> {
        self.connection.execute("INSERT OR REPLACE INTO fingerprints (oid, integrity, fingerprint) VALUES (?1, ?2, ?3)", params![oid, integrity, fingerprint]).map_err(cache_error)?;
        Ok(())
    }
}

fn cache_error(error: impl std::fmt::Display) -> AppError {
    AppError::operational(format!("error: patch relationship cache: {error}"))
}
