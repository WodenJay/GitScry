//! Vector maintenance, migration and retrieval for the fixed semantic encoder.
mod migration;
mod retrieval;
mod vectors;

pub(super) use migration::copy_semantic_vectors;
pub(crate) use retrieval::SemanticCandidate;
pub(super) use retrieval::semantic_top_k;
use std::{collections::HashMap, path::Path};
use vectors::{valid_embedding, valid_fingerprint};

use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use sha2::{Digest, Sha256};

use crate::{
    app::{AppError, IndexStage},
    semantic::{
        self, CommitDocument, EMBEDDING_BATCH_SIZE, Encoder, InputPreprocessor, PreparedInput,
    },
};

use super::cache_error;

const PAGE_SIZE: usize = 256;

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
    let mut preprocessor = None;
    let ready = match is_ready(
        &connection,
        &tip,
        commit_count,
        &encoder_fingerprint,
        &mut preprocessor,
    ) {
        Ok(ready) => ready,
        Err(error) => {
            set_metadata_transaction(
                &mut connection,
                &[("semantic_enabled", "1"), ("semantic_ready", "0")],
            )?;
            return Err(error);
        }
    };
    if ready {
        return Ok(());
    }
    set_metadata_transaction(
        &mut connection,
        &[("semantic_enabled", "1"), ("semantic_ready", "0")],
    )?;
    if matches!(preference, SemanticPreference::Preserve)
        && has_incompatible_encoder(&connection, &encoder_fingerprint, commit_count)?
    {
        return Err(AppError::operational(
            "error: semantic vectors use a different encoder; the ordinary history cache is usable and semantic indexing remains enabled; rerun `gitscry index --semantic` to rebuild them",
        ));
    }

    let runtime_provenance = maintain_vectors(
        &mut connection,
        &encoder_fingerprint,
        report,
        &mut preprocessor,
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

fn commit_document(message: &[u8], paths: &[Vec<u8>]) -> CommitDocument {
    let (title, body) = super::message_parts(message);
    CommitDocument {
        title,
        body,
        paths: paths
            .iter()
            .map(|path| String::from_utf8_lossy(path).into_owned())
            .collect(),
    }
}

fn ready_metadata_matches(
    connection: &Connection,
    tip: &str,
    commit_count: i64,
    encoder_fingerprint: &str,
) -> Result<bool, AppError> {
    let coverage_count = commit_count.to_string();
    Ok(
        metadata(connection, "semantic_ready")?.as_deref() == Some("1")
            && metadata(connection, "semantic_coverage_tip")?.as_deref() == Some(tip)
            && metadata(connection, "semantic_coverage_count")?.as_deref()
                == Some(coverage_count.as_str())
            && metadata(connection, "semantic_encoder_fingerprint")?.as_deref()
                == Some(encoder_fingerprint)
            && count_vectors(connection)? == commit_count,
    )
}

fn is_ready(
    connection: &Connection,
    tip: &str,
    commit_count: i64,
    encoder_fingerprint: &str,
    preprocessor: &mut Option<InputPreprocessor>,
) -> Result<bool, AppError> {
    if !ready_metadata_matches(connection, tip, commit_count, encoder_fingerprint)? {
        return Ok(false);
    }
    let mut last_position = -1_i64;
    loop {
        let page = read_page(connection, last_position)?;
        if page.is_empty() {
            break;
        }
        last_position = page.last().expect("page is non-empty").position;
        let commit_ids = page.iter().map(|commit| commit.id).collect::<Vec<_>>();
        let mut paths = paths_for_page(connection, &commit_ids)?;
        for commit in page {
            let message = super::decode_message(&commit.message, commit.message_length)?;
            let commit_paths = paths.remove(&commit.id).unwrap_or_default();
            let current_source_fingerprint = source_fingerprint(&message, &commit_paths);
            if !reusable_vector(&commit, encoder_fingerprint)
                || commit.source_fingerprint.as_deref() != Some(current_source_fingerprint.as_str())
            {
                return Ok(false);
            }
            let document = commit_document(&message, &commit_paths);
            if preprocessor.is_none() {
                *preprocessor = Some(InputPreprocessor::load()?);
            }
            let prepared = preprocessor
                .as_ref()
                .expect("preprocessor was initialized")
                .prepare(&document)?;
            if commit.input_fingerprint.as_deref() != Some(prepared.fingerprint()) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn reusable_vector(commit: &CachedCommit, encoder_fingerprint: &str) -> bool {
    commit.cached_commit_oid.as_deref() == Some(commit.oid.as_str())
        && valid_embedding(commit.embedding.as_deref())
        && commit.encoder_fingerprint.as_deref() == Some(encoder_fingerprint)
        && valid_fingerprint(commit.input_fingerprint.as_deref())
}

fn has_incompatible_encoder(
    connection: &Connection,
    encoder_fingerprint: &str,
    commit_count: i64,
) -> Result<bool, AppError> {
    let stored_fingerprint = metadata(connection, "semantic_encoder_fingerprint")?;
    let stale_metadata = stored_fingerprint
        .as_deref()
        .is_some_and(|stored| !stored.is_empty() && stored != encoder_fingerprint);
    let incompatible_vectors = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM semantic_vectors WHERE encoder_fingerprint != ?1
            )",
            [encoder_fingerprint],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| cache_error("checking semantic encoder compatibility", error))?;
    let incomplete = count_vectors(connection)? < commit_count;
    Ok(incompatible_vectors || (stale_metadata && incomplete))
}

fn report_semantic_index(report: &mut dyn FnMut(IndexStage), reported: &mut bool) {
    if !*reported {
        report(IndexStage::BuildingSemanticIndex);
        *reported = true;
    }
}

pub(super) fn require_ready_for_query(connection: &Connection) -> Result<(), AppError> {
    if !semantic_enabled(connection)? {
        return Err(AppError::operational(
            "error: semantic search is disabled; run `gitscry index --semantic` while online to enable it. Ordinary history search remains available.",
        ));
    }
    let (tip, commit_count) = completed_generation(connection)?;
    let encoder_fingerprint = semantic::encoder_fingerprint();
    if !ready_metadata_matches(connection, &tip, commit_count, &encoder_fingerprint)? {
        return Err(AppError::operational(
            "error: semantic index is missing, stale, or incomplete; run `gitscry index --semantic` while online to repair it. Ordinary history search remains available.",
        ));
    }
    let linked_count = connection
        .query_row(
            "SELECT count(*) FROM semantic_vectors
             JOIN commits USING (commit_id)",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| super::cache_error("checking semantic vector identities", error))?;
    if linked_count != commit_count {
        return Err(AppError::operational(
            "error: semantic index is missing, stale, or incomplete; run `gitscry index --semantic` while online to repair it. Ordinary history search remains available.",
        ));
    }
    Ok(())
}

fn maintain_vectors(
    connection: &mut Connection,
    encoder_fingerprint: &str,
    report: &mut dyn FnMut(IndexStage),
    preprocessor: &mut Option<InputPreprocessor>,
) -> Result<Option<String>, AppError> {
    let mut last_position = -1_i64;
    let mut encoder = None;
    let mut stage_reported = false;
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
            let vector_is_reusable = reusable_vector(&commit, encoder_fingerprint);
            let document = commit_document(&message, &commit_paths);

            if encoder.is_none() && preprocessor.is_none() {
                report_semantic_index(report, &mut stage_reported);
                if vector_is_reusable {
                    *preprocessor = Some(InputPreprocessor::load()?);
                } else {
                    encoder = Some(Encoder::load(None)?);
                }
            }
            let prepared = if let Some(preprocessor) = preprocessor.as_ref() {
                preprocessor.prepare(&document)?
            } else {
                encoder
                    .as_ref()
                    .expect("encoder or preprocessor was initialized")
                    .prepare(&document)?
            };
            if vector_is_reusable
                && commit.input_fingerprint.as_deref() == Some(prepared.fingerprint())
            {
                if commit.source_fingerprint.as_deref() != Some(source_fingerprint.as_str()) {
                    source_updates.push(SourceFingerprintUpdate {
                        commit_id: commit.id,
                        source_fingerprint,
                    });
                }
                continue;
            }
            page_vectors.push(PendingVector {
                commit_id: commit.id,
                commit_oid: commit.oid,
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
                preprocessor,
                &mut batch,
                encoder_fingerprint,
            )?;
        }
        flush_source_updates(connection, &mut source_updates)?;
    }
    Ok(encoder.map(|encoder: Encoder| encoder.runtime_provenance().to_owned()))
}

struct PendingVector {
    commit_id: i64,
    commit_oid: String,
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
    oid: String,
    message: Vec<u8>,
    message_length: i64,
    cached_commit_oid: Option<String>,
    embedding: Option<Vec<u8>>,
    source_fingerprint: Option<String>,
    input_fingerprint: Option<String>,
    encoder_fingerprint: Option<String>,
}
fn read_page(connection: &Connection, after_position: i64) -> Result<Vec<CachedCommit>, AppError> {
    let mut statement = connection
        .prepare(
            "SELECT c.commit_id, c.position, c.oid, c.message, c.message_length,
                    v.commit_oid, v.embedding, v.source_fingerprint,
                    v.input_fingerprint, v.encoder_fingerprint
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
                oid: row.get(2)?,
                message: row.get(3)?,
                message_length: row.get(4)?,
                cached_commit_oid: row.get(5)?,
                embedding: row.get(6)?,
                source_fingerprint: row.get(7)?,
                input_fingerprint: row.get(8)?,
                encoder_fingerprint: row.get(9)?,
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
    preprocessor: &mut Option<InputPreprocessor>,
    batch: &mut Vec<PendingVector>,
    encoder_fingerprint: &str,
) -> Result<(), AppError> {
    if batch.is_empty() {
        return Ok(());
    }
    if encoder.is_none() {
        *encoder = Some(Encoder::load(preprocessor.take())?);
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
        let embedding = vectors::encode(&vector)?;
        transaction
            .execute(
                "INSERT INTO semantic_vectors(
                    commit_id, commit_oid, source_fingerprint, embedding, input_fingerprint,
                    encoder_fingerprint, runtime_provenance
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(commit_id) DO UPDATE SET
                    commit_oid = excluded.commit_oid,
                    source_fingerprint = excluded.source_fingerprint,
                    embedding = excluded.embedding,
                    input_fingerprint = excluded.input_fingerprint,
                    encoder_fingerprint = excluded.encoder_fingerprint,
                    runtime_provenance = excluded.runtime_provenance",
                params![
                    pending.commit_id,
                    pending.commit_oid,
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

#[cfg(test)]
mod tests {
    use super::*;

    use tokenizers::{
        Tokenizer, models::wordlevel::WordLevel, pre_tokenizers::whitespace::Whitespace,
    };

    fn test_preprocessor() -> InputPreprocessor {
        let vocabulary = [("[UNK]".to_owned(), 0), ("[PAD]".to_owned(), 1)]
            .into_iter()
            .collect();
        let model = WordLevel::builder()
            .vocab(vocabulary)
            .unk_token("[UNK]".to_owned())
            .build()
            .unwrap();
        let mut tokenizer = Tokenizer::new(model);
        tokenizer.with_pre_tokenizer(Some(Whitespace));
        InputPreprocessor::from_tokenizer(tokenizer).unwrap()
    }
    #[test]
    fn ready_semantic_index_validates_vectors_not_runtime_provenance() {
        let directory = tempfile::tempdir().unwrap();
        let cache_directory = directory.path().join(".gitscry");
        std::fs::create_dir_all(&cache_directory).unwrap();
        let cache_path = cache_directory.join("cache.sqlite");
        let connection = Connection::open(&cache_path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE TABLE commits (
                    commit_id INTEGER PRIMARY KEY, position INTEGER, oid TEXT,
                    message BLOB, message_length INTEGER, commit_time INTEGER DEFAULT 1
                 );
                 CREATE TABLE semantic_vectors (
                    commit_id INTEGER PRIMARY KEY, commit_oid TEXT, embedding BLOB,
                    source_fingerprint TEXT, input_fingerprint TEXT,
                    encoder_fingerprint TEXT, runtime_provenance TEXT
                 );
                 CREATE TABLE commit_paths (
                    commit_id INTEGER, path_order INTEGER, raw_path BLOB
                 );",
            )
            .unwrap();
        let message = b"commit title";
        let compressed = lz4_flex::compress(message);
        let paths = vec![b"src/lib.rs".to_vec()];
        connection
            .execute(
                "INSERT INTO commits VALUES (1, 0, 'oid', ?1, ?2, 1)",
                params![compressed, message.len() as i64],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO commit_paths VALUES (1, 0, ?1)",
                params![paths[0].as_slice()],
            )
            .unwrap();
        let expected_source_fingerprint = source_fingerprint(message, &paths);

        let encoder_fingerprint = semantic::encoder_fingerprint();
        for (key, value) in [
            ("semantic_enabled", "1"),
            ("semantic_ready", "1"),
            ("semantic_coverage_tip", "tip"),
            ("semantic_coverage_count", "1"),
            ("semantic_encoder_fingerprint", encoder_fingerprint.as_str()),
            ("semantic_runtime_provenance", "previous-runtime"),
            ("completed_tip", "tip"),
            ("completed_commit_count", "1"),
        ] {
            connection
                .execute(
                    "INSERT INTO metadata(key, value) VALUES (?1, ?2)",
                    params![key, value],
                )
                .unwrap();
        }
        let mut embedding = vec![0.0_f32; 384];
        embedding[0] = 1.0;
        let embedding = embedding
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>();
        connection
            .execute(
                "INSERT INTO semantic_vectors(
                    commit_id, commit_oid, embedding, source_fingerprint, input_fingerprint,
                    encoder_fingerprint, runtime_provenance
                 ) VALUES (1, 'oid', ?1, ?2, ?3, ?4, 'previous-runtime')",
                params![
                    embedding,
                    expected_source_fingerprint,
                    "b".repeat(64),
                    encoder_fingerprint
                ],
            )
            .unwrap();
        drop(connection);

        let mut connection = Connection::open(cache_path).unwrap();
        let mut preprocessor = Some(test_preprocessor());
        let document = commit_document(message, &paths);
        let input_fingerprint = preprocessor
            .as_ref()
            .unwrap()
            .prepare(&document)
            .unwrap()
            .fingerprint()
            .to_owned();
        connection
            .execute(
                "UPDATE semantic_vectors SET input_fingerprint = ?1",
                [&input_fingerprint],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE metadata SET value = 'new-runtime' WHERE key = 'semantic_runtime_provenance'",
                [],
            )
            .unwrap();
        assert!(
            maintain_vectors(
                &mut connection,
                &encoder_fingerprint,
                &mut |_| {},
                &mut preprocessor,
            )
            .unwrap()
            .is_none()
        );
        assert!(
            is_ready(
                &connection,
                "tip",
                1,
                &encoder_fingerprint,
                &mut preprocessor,
            )
            .unwrap()
        );
        assert_eq!(
            metadata(&connection, "semantic_runtime_provenance")
                .unwrap()
                .as_deref(),
            Some("new-runtime")
        );
        let vector_runtime_provenance = connection
            .query_row(
                "SELECT runtime_provenance FROM semantic_vectors WHERE commit_id = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert_eq!(vector_runtime_provenance, "previous-runtime");

        // A rebuilt generation changes row IDs, but compatible vectors remain
        // reusable and queryable without loading a runtime or re-encoding.
        let staging = directory.path().join("staging.sqlite");
        connection
            .execute("VACUUM INTO ?1", [staging.to_str().unwrap()])
            .unwrap();
        let mut rebuilt = Connection::open(&staging).unwrap();
        rebuilt
            .execute_batch(
                "DELETE FROM semantic_vectors;
             UPDATE commits SET commit_id = 2;
             UPDATE commit_paths SET commit_id = 2;
             UPDATE metadata SET value = '0' WHERE key = 'semantic_ready';",
            )
            .unwrap();
        copy_semantic_vectors(&cache_directory.join("cache.sqlite"), &staging);
        assert!(require_ready_for_query(&rebuilt).is_err());
        assert!(
            maintain_vectors(
                &mut rebuilt,
                &encoder_fingerprint,
                &mut |_| {},
                &mut preprocessor
            )
            .unwrap()
            .is_none()
        );
        set_metadata(&rebuilt, &[("semantic_ready", "1")]).unwrap();
        assert!(is_ready(&rebuilt, "tip", 1, &encoder_fingerprint, &mut preprocessor).unwrap());
        require_ready_for_query(&rebuilt).unwrap();
        let mut query = vec![0.0; crate::semantic::EMBEDDING_DIMENSION];
        query[0] = 1.0;
        let hits = semantic_top_k(&rebuilt, &[query], 1, None).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].commit_id, 2);
        assert_eq!(hits[0].oid, "oid");
        assert_eq!(hits[0].cosine, 1.0);
        connection
            .execute(
                "UPDATE semantic_vectors SET input_fingerprint = ?1",
                ["b".repeat(64)],
            )
            .unwrap();
        assert!(
            !is_ready(
                &connection,
                "tip",
                1,
                &encoder_fingerprint,
                &mut preprocessor,
            )
            .unwrap(),
            "a well-formed but incorrect input fingerprint must not establish readiness"
        );
        connection
            .execute("UPDATE semantic_vectors SET commit_oid = 'other'", [])
            .unwrap();
        assert!(
            !is_ready(
                &connection,
                "tip",
                1,
                &encoder_fingerprint,
                &mut preprocessor,
            )
            .unwrap()
        );
        connection
            .execute(
                "UPDATE semantic_vectors SET commit_oid = 'oid', source_fingerprint = ?1",
                ["d".repeat(64)],
            )
            .unwrap();
        assert!(
            !is_ready(
                &connection,
                "tip",
                1,
                &encoder_fingerprint,
                &mut preprocessor,
            )
            .unwrap()
        );
    }
}
