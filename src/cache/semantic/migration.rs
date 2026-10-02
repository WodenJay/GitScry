//! Best-effort vector reuse when publishing an ordinary cache generation.
use super::vectors::EMBEDDING_BYTES;
use rusqlite::Connection;
use std::path::Path;
pub(in crate::cache) fn copy_semantic_vectors(published: &Path, staging: &Path) {
    if !published.is_file() {
        return;
    }
    let Ok(connection) = Connection::open(staging) else {
        return;
    };
    let published = published.to_string_lossy();
    if connection
        .execute("ATTACH DATABASE ?1 AS previous", [published.as_ref()])
        .is_err()
    {
        return;
    }
    let _ = connection.execute(
        "UPDATE main.metadata
         SET value = (SELECT value FROM previous.metadata WHERE key = 'semantic_encoder_fingerprint')
         WHERE key = 'semantic_encoder_fingerprint'
           AND EXISTS (SELECT 1 FROM previous.metadata WHERE key = 'semantic_encoder_fingerprint')",
        [],
    );
    let _ = connection.execute(
        "UPDATE main.metadata
         SET value = (SELECT value FROM previous.metadata WHERE key = 'semantic_runtime_provenance')
         WHERE key = 'semantic_runtime_provenance'
           AND EXISTS (SELECT 1 FROM previous.metadata WHERE key = 'semantic_runtime_provenance')",
        [],
    );
    if connection.execute_batch("BEGIN IMMEDIATE").is_err() {
        let _ = connection.execute_batch("DETACH DATABASE previous");
        return;
    }
    let has_commit_oid = match connection.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM pragma_table_info('semantic_vectors', 'previous')
            WHERE name = 'commit_oid'
        )",
        [],
        |row| row.get::<_, bool>(0),
    ) {
        Ok(has_commit_oid) => has_commit_oid,
        Err(_) => {
            let _ = connection.execute_batch("ROLLBACK");
            let _ = connection.execute_batch("DETACH DATABASE previous");
            return;
        }
    };
    let identity_check = if has_commit_oid {
        " AND vector.commit_oid = old_commit.oid"
    } else {
        ""
    };
    let query = format!(
        "INSERT INTO main.semantic_vectors(
            commit_id, commit_oid, embedding, source_fingerprint, input_fingerprint,
            encoder_fingerprint, runtime_provenance
         )
         SELECT current.commit_id, current.oid, vector.embedding,
                vector.source_fingerprint, vector.input_fingerprint,
                vector.encoder_fingerprint, vector.runtime_provenance
         FROM previous.semantic_vectors AS vector
         JOIN previous.commits AS old_commit ON old_commit.commit_id = vector.commit_id
         JOIN main.commits AS current ON current.oid = old_commit.oid
         WHERE length(vector.embedding) = {EMBEDDING_BYTES}
           AND length(vector.source_fingerprint) = 64
           AND length(vector.input_fingerprint) = 64
           AND length(vector.encoder_fingerprint) = 64{identity_check}"
    );
    let vectors_copied = connection.execute(&query, []).is_ok();
    if vectors_copied {
        let _ = connection.execute_batch("COMMIT");
    } else {
        let _ = connection.execute_batch("ROLLBACK");
    }
    let _ = connection.execute_batch("DETACH DATABASE previous");
}
