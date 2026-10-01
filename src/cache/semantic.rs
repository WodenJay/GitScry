use std::{collections::HashMap, path::Path};

use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use sha2::{Digest, Sha256};

use crate::{
    app::{AppError, IndexStage},
    semantic::{self, CommitDocument, Encoder, PreparedInput},
};

use super::cache_error;

const PAGE_SIZE: usize = 256;
const EMBEDDING_BATCH_SIZE: usize = 8;
const EMBEDDING_BYTES: usize = 384 * std::mem::size_of::<f32>();

#[derive(Clone, Copy)]
pub(crate) enum SemanticPreference {
    Preserve,
    Enable,
    Disable,
}

pub(crate) fn maintain(
    root: &Path,
    preference: SemanticPreference,
    report: &mut dyn FnMut(IndexStage),
) -> Result<(), AppError> {
    let mut progress = Vec::new();
    let _lock = super::acquire_exclusive(root, &mut progress)?;
    let path = root.join(".gitscry/cache.sqlite");
    let mut connection = Connection::open(&path)
        .map_err(|error| cache_error("opening cache for semantic indexing", error))?;
    connection
        .execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = FULL;")
        .map_err(|error| cache_error("configuring semantic cache", error))?;

    if matches!(preference, SemanticPreference::Disable) {
        disable(&mut connection)?;
        return Ok(());
    }

    let enabled = semantic_enabled(&connection)?;
    if matches!(preference, SemanticPreference::Enable) && !enabled {
        set_metadata_transaction(
            &mut connection,
            &[("semantic_enabled", "1"), ("semantic_ready", "0")],
        )?;
    } else if !enabled {
        return Ok(());
    }

    let (tip, commit_count) = completed_generation(&connection)?;
    let encoder_fingerprint = semantic::encoder_fingerprint();
    if is_ready(&connection, &tip, commit_count, &encoder_fingerprint)? {
        return Ok(());
    }
    set_metadata_transaction(
        &mut connection,
        &[("semantic_enabled", "1"), ("semantic_ready", "0")],
    )?;

    let runtime_provenance = maintain_vectors(
        &mut connection,
        &encoder_fingerprint,
        report,
    )
    .map_err(|error| {
        AppError::operational(format!(
            "{error}; the ordinary history cache is usable, semantic indexing remains enabled, and you can retry with `gitscry index` or disable it with `gitscry index --no-semantic`."
        ))
    })?;

    let vector_count = count_vectors(&connection)?;
    if vector_count != commit_count {
        return Err(AppError::operational(format!(
            "error: semantic index covers {vector_count} of {commit_count} commits; retry `gitscry index`"
        )));
    }
    let runtime_provenance = runtime_provenance
        .or(metadata(&connection, "semantic_runtime_provenance")?)
        .or(latest_runtime_provenance(&connection)?)
        .unwrap_or_else(|| "unknown".to_owned());
    let coverage_count = commit_count.to_string();
    set_metadata_transaction(
        &mut connection,
        &[
            ("semantic_enabled", "1"),
            ("semantic_ready", "1"),
            ("semantic_coverage_tip", tip.as_str()),
            ("semantic_coverage_count", coverage_count.as_str()),
            ("semantic_encoder_fingerprint", encoder_fingerprint.as_str()),
            ("semantic_runtime_provenance", runtime_provenance.as_str()),
        ],
    )?;
    Ok(())
}

fn disable(connection: &mut Connection) -> Result<(), AppError> {
    let transaction = connection
        .transaction()
        .map_err(|error| cache_error("starting semantic disable transaction", error))?;
    set_metadata(
        &transaction,
        &[
            ("semantic_enabled", "0"),
            ("semantic_ready", "0"),
            ("semantic_coverage_tip", ""),
            ("semantic_coverage_count", "0"),
            ("semantic_encoder_fingerprint", ""),
            ("semantic_runtime_provenance", ""),
        ],
    )?;
    transaction
        .execute("DELETE FROM semantic_vectors", [])
        .map_err(|error| cache_error("removing semantic vectors", error))?;
    transaction
        .commit()
        .map_err(|error| cache_error("committing semantic disable", error))
}

fn semantic_enabled(connection: &Connection) -> Result<bool, AppError> {
    match metadata(connection, "semantic_enabled")?.as_deref() {
        Some("0") => Ok(false),
        Some("1") => Ok(true),
        _ => Err(AppError::operational(
            "error: semantic enablement metadata is invalid; rerun `gitscry index`",
        )),
    }
}

fn completed_generation(connection: &Connection) -> Result<(String, i64), AppError> {
    let tip = metadata(connection, "completed_tip")?.ok_or_else(|| {
        AppError::operational("error: ordinary cache has no completed tip; rerun `gitscry index`")
    })?;
    let count = metadata(connection, "completed_commit_count")?
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or_else(|| {
            AppError::operational(
                "error: ordinary cache has invalid completion metadata; rerun `gitscry index`",
            )
        })?;
    Ok((tip, count))
}

fn is_ready(
    connection: &Connection,
    tip: &str,
    commit_count: i64,
    encoder_fingerprint: &str,
) -> Result<bool, AppError> {
    if metadata(connection, "semantic_ready")?.as_deref() != Some("1")
        || metadata(connection, "semantic_coverage_tip")?.as_deref() != Some(tip)
        || metadata(connection, "semantic_coverage_count")?.as_deref()
            != Some(commit_count.to_string().as_str())
        || metadata(connection, "semantic_encoder_fingerprint")?.as_deref()
            != Some(encoder_fingerprint)
    {
        return Ok(false);
    }
    Ok(count_vectors(connection)? == commit_count)
}

fn maintain_vectors(
    connection: &mut Connection,
    encoder_fingerprint: &str,
    report: &mut dyn FnMut(IndexStage),
) -> Result<Option<String>, AppError> {
    let mut last_position = -1_i64;
    let mut encoder = None;
    loop {
        let page = read_page(connection, last_position)?;
        if page.is_empty() {
            break;
        }
        last_position = page.last().expect("page is non-empty").position;
        let commit_ids = page.iter().map(|commit| commit.id).collect::<Vec<_>>();
        let mut paths = paths_for_page(connection, &commit_ids)?;

        let mut page_vectors = Vec::with_capacity(page.len());
        let mut source_updates = Vec::new();
        for commit in page {
            let message = super::decode_message(&commit.message, commit.message_length)?;
            let commit_paths = paths.remove(&commit.id).unwrap_or_default();
            let source_fingerprint = source_fingerprint(&message, &commit_paths);
            if commit.source_fingerprint.as_deref() == Some(source_fingerprint.as_str())
                && commit.encoder_fingerprint.as_deref() == Some(encoder_fingerprint)
            {
                continue;
            }

            let (title, body) = super::message_parts(&message);
            let document = CommitDocument {
                title,
                body,
                paths: commit_paths
                    .iter()
                    .map(|path| String::from_utf8_lossy(path).into_owned())
                    .collect(),
            };
            if encoder.is_none() {
                report(IndexStage::BuildingSemanticIndex);
                encoder = Some(Encoder::load()?);
            }
            let prepared = encoder
                .as_ref()
                .expect("encoder was initialized")
                .prepare(&document)?;
            if commit.encoder_fingerprint.as_deref() == Some(encoder_fingerprint)
                && commit.input_fingerprint.as_deref() == Some(prepared.fingerprint())
            {
                source_updates.push(SourceFingerprintUpdate {
                    commit_id: commit.id,
                    source_fingerprint,
                });
                continue;
            }
            page_vectors.push(PendingVector {
                commit_id: commit.id,
                source_fingerprint,
                input_fingerprint: prepared.fingerprint().to_owned(),
                input: prepared,
            });
        }

        // Group nearby sequence lengths to limit right-padding in each batch.
        page_vectors.sort_by_key(|pending| pending.input.token_count());
        while !page_vectors.is_empty() {
            let batch_size = page_vectors.len().min(EMBEDDING_BATCH_SIZE);
            let mut batch = page_vectors.drain(..batch_size).collect::<Vec<_>>();
            flush_batch(
                connection,
                &mut encoder,
                &mut batch,
                encoder_fingerprint,
                report,
            )?;
        }
        flush_source_updates(connection, &mut source_updates)?;
    }
    Ok(encoder.map(|encoder: Encoder| encoder.runtime_provenance().to_owned()))
}

struct PendingVector {
    commit_id: i64,
    source_fingerprint: String,
    input_fingerprint: String,
    input: PreparedInput,
}

struct SourceFingerprintUpdate {
    commit_id: i64,
    source_fingerprint: String,
}

struct CachedCommit {
    id: i64,
    position: i64,
    message: Vec<u8>,
    message_length: i64,
    source_fingerprint: Option<String>,
    input_fingerprint: Option<String>,
    encoder_fingerprint: Option<String>,
}

fn read_page(connection: &Connection, after_position: i64) -> Result<Vec<CachedCommit>, AppError> {
    let mut statement = connection
        .prepare(
            "SELECT c.commit_id, c.position, c.message, c.message_length,
                    v.source_fingerprint, v.input_fingerprint, v.encoder_fingerprint
             FROM commits AS c
             LEFT JOIN semantic_vectors AS v ON v.commit_id = c.commit_id
             WHERE c.position > ?1
             ORDER BY c.position
             LIMIT ?2",
        )
        .map_err(|error| cache_error("preparing semantic commit page", error))?;
    statement
        .query_map(params![after_position, PAGE_SIZE as i64], |row| {
            Ok(CachedCommit {
                id: row.get(0)?,
                position: row.get(1)?,
                message: row.get(2)?,
                message_length: row.get(3)?,
                source_fingerprint: row.get(4)?,
                input_fingerprint: row.get(5)?,
                encoder_fingerprint: row.get(6)?,
            })
        })
        .map_err(|error| cache_error("reading semantic commit page", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| cache_error("reading semantic commit page", error))
}

fn paths_for_page(
    connection: &Connection,
    commit_ids: &[i64],
) -> Result<HashMap<i64, Vec<Vec<u8>>>, AppError> {
    let placeholders = std::iter::repeat_n("?", commit_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let query = format!(
        "SELECT commit_id, raw_path FROM commit_paths
         WHERE commit_id IN ({placeholders})
         ORDER BY commit_id, path_order"
    );
    let mut statement = connection
        .prepare(&query)
        .map_err(|error| cache_error("preparing semantic path lookup", error))?;
    let rows = statement
        .query_map(params_from_iter(commit_ids.iter()), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(|error| cache_error("reading semantic commit paths", error))?;
    let mut paths = HashMap::<i64, Vec<Vec<u8>>>::new();
    for row in rows {
        let (commit_id, path) =
            row.map_err(|error| cache_error("reading semantic commit paths", error))?;
        paths.entry(commit_id).or_default().push(path);
    }
    Ok(paths)
}

fn flush_batch(
    connection: &mut Connection,
    encoder: &mut Option<Encoder>,
    batch: &mut Vec<PendingVector>,
    encoder_fingerprint: &str,
    report: &mut dyn FnMut(IndexStage),
) -> Result<(), AppError> {
    if batch.is_empty() {
        return Ok(());
    }
    if encoder.is_none() {
        report(IndexStage::BuildingSemanticIndex);
        *encoder = Some(Encoder::load()?);
    }
    let encoder = encoder.as_mut().expect("encoder was initialized");
    let inputs = batch.iter().map(|item| &item.input).collect::<Vec<_>>();
    let vectors = encoder.embed(&inputs)?;
    if vectors.len() != batch.len() {
        return Err(AppError::operational(
            "error: semantic encoder returned an unexpected batch size",
        ));
    }
    let runtime_provenance = encoder.runtime_provenance();
    let transaction = connection
        .transaction()
        .map_err(|error| cache_error("starting semantic vector transaction", error))?;
    for (pending, vector) in batch.iter().zip(vectors) {
        if vector.len() != 384 || vector.iter().any(|value| !value.is_finite()) {
            return Err(AppError::operational(
                "error: semantic encoder returned an invalid 384-dimensional vector",
            ));
        }
        let mut embedding = Vec::with_capacity(EMBEDDING_BYTES);
        for value in vector {
            embedding.extend_from_slice(&value.to_le_bytes());
        }
        transaction
            .execute(
                "INSERT INTO semantic_vectors(
                    commit_id, source_fingerprint, embedding, input_fingerprint,
                    encoder_fingerprint, runtime_provenance
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(commit_id) DO UPDATE SET
                    source_fingerprint = excluded.source_fingerprint,
                    embedding = excluded.embedding,
                    input_fingerprint = excluded.input_fingerprint,
                    encoder_fingerprint = excluded.encoder_fingerprint,
                    runtime_provenance = excluded.runtime_provenance",
                params![
                    pending.commit_id,
                    pending.source_fingerprint,
                    embedding,
                    pending.input_fingerprint,
                    encoder_fingerprint,
                    runtime_provenance,
                ],
            )
            .map_err(|error| cache_error("writing semantic vector", error))?;
    }
    transaction
        .commit()
        .map_err(|error| cache_error("committing semantic vectors", error))?;
    batch.clear();
    Ok(())
}

fn flush_source_updates(
    connection: &mut Connection,
    updates: &mut Vec<SourceFingerprintUpdate>,
) -> Result<(), AppError> {
    if updates.is_empty() {
        return Ok(());
    }
    let transaction = connection
        .transaction()
        .map_err(|error| cache_error("starting semantic source update transaction", error))?;
    for update in updates.iter() {
        let changed = transaction
            .execute(
                "UPDATE semantic_vectors SET source_fingerprint = ?1 WHERE commit_id = ?2",
                params![update.source_fingerprint, update.commit_id],
            )
            .map_err(|error| cache_error("updating semantic source fingerprint", error))?;
        if changed != 1 {
            return Err(AppError::operational(
                "error: semantic vector disappeared while updating its source fingerprint",
            ));
        }
    }
    transaction
        .commit()
        .map_err(|error| cache_error("committing semantic source updates", error))?;
    updates.clear();
    Ok(())
}

fn source_fingerprint(message: &[u8], paths: &[Vec<u8>]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"gitscry-semantic-source-v1\0");
    hasher.update((message.len() as u64).to_le_bytes());
    hasher.update(message);
    hasher.update((paths.len() as u64).to_le_bytes());
    for path in paths {
        hasher.update((path.len() as u64).to_le_bytes());
        hasher.update(path);
    }
    format!("{:x}", hasher.finalize())
}

fn count_vectors(connection: &Connection) -> Result<i64, AppError> {
    connection
        .query_row("SELECT COUNT(*) FROM semantic_vectors", [], |row| {
            row.get(0)
        })
        .map_err(|error| cache_error("counting semantic vectors", error))
}

fn latest_runtime_provenance(connection: &Connection) -> Result<Option<String>, AppError> {
    connection
        .query_row(
            "SELECT runtime_provenance FROM semantic_vectors
             ORDER BY commit_id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| cache_error("reading semantic runtime provenance", error))
}

fn metadata(connection: &Connection, key: &str) -> Result<Option<String>, AppError> {
    connection
        .query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|error| cache_error("reading semantic cache metadata", error))
}

fn set_metadata_transaction(
    connection: &mut Connection,
    values: &[(&str, &str)],
) -> Result<(), AppError> {
    let transaction = connection
        .transaction()
        .map_err(|error| cache_error("starting semantic metadata transaction", error))?;
    set_metadata(&transaction, values)?;
    transaction
        .commit()
        .map_err(|error| cache_error("committing semantic metadata", error))
}

fn set_metadata(connection: &Connection, values: &[(&str, &str)]) -> Result<(), AppError> {
    for (key, value) in values {
        connection
            .execute(
                "INSERT INTO metadata(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(|error| cache_error("writing semantic cache metadata", error))?;
    }
    Ok(())
}
