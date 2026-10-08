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
    pub(crate) forward_fingerprint: Option<Vec<u8>>,
    pub(crate) inverse_fingerprint: Option<Vec<u8>>,
    pub(crate) objects: String,
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
        let stored: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(cache_error)?;
        if stored != version {
            connection
                .execute_batch(&format!(
                    "BEGIN; DROP TABLE IF EXISTS fingerprints; PRAGMA user_version={version}; COMMIT;"
                ))
                .map_err(cache_error)?;
        }
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS fingerprints (oid TEXT PRIMARY KEY, integrity TEXT NOT NULL, forward_fingerprint BLOB, inverse_fingerprint BLOB, objects TEXT NOT NULL)",
            )
            .map_err(cache_error)?;
        Ok(Self {
            connection,
            _lock: lock,
        })
    }

    pub(crate) fn get(&self, oid: &str) -> Result<Option<PatchFingerprint>, AppError> {
        self.connection
            .query_row(
                "SELECT integrity, forward_fingerprint, inverse_fingerprint, objects FROM fingerprints WHERE oid=?1",
                [oid],
                |row| {
                    Ok(PatchFingerprint {
                        integrity: row.get(0)?,
                        forward_fingerprint: row.get(1)?,
                        inverse_fingerprint: row.get(2)?,
                        objects: row.get(3)?,
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
        forward_fingerprint: Option<&[u8]>,
        inverse_fingerprint: Option<&[u8]>,
        objects: &[String],
    ) -> Result<(), AppError> {
        self.connection
            .execute(
                "INSERT OR REPLACE INTO fingerprints (oid, integrity, forward_fingerprint, inverse_fingerprint, objects) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    oid,
                    integrity,
                    forward_fingerprint,
                    inverse_fingerprint,
                    objects.join("\n")
                ],
            )
            .map_err(cache_error)?;
        Ok(())
    }
}

fn cache_error(error: impl std::fmt::Display) -> AppError {
    AppError::operational(format!("error: patch relationship cache: {error}"))
}
