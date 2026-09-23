use std::collections::HashMap;

use lz4_flex::{compress, decompress};
use rusqlite::{Connection, Transaction, params};

use crate::{app::AppError, git::Hunk};

const MAX_HUNK_BYTES: usize = 1 << 30;
#[cfg(not(test))]
const BLOCK_BYTES: usize = 8 * 1024 * 1024;
#[cfg(test)]
const BLOCK_BYTES: usize = 64;

pub(super) fn encode(bytes: &[u8]) -> (Vec<u8>, i64) {
    (compress(bytes), bytes.len() as i64)
}

pub(super) fn decode(bytes: &[u8], length: i64, material: &str) -> Result<Vec<u8>, AppError> {
    let length = checked_length(length, material)?;
    let decoded =
        decompress(bytes, length).map_err(|error| corruption(material, error.to_string()))?;
    if decoded.len() != length {
        return Err(corruption(
            material,
            format!("decoded length {} does not match {length}", decoded.len()),
        ));
    }
    Ok(decoded)
}

fn checked_length(length: i64, material: &str) -> Result<usize, AppError> {
    let length = usize::try_from(length).map_err(|_| corruption(material, "invalid length"))?;
    if length > MAX_HUNK_BYTES {
        return Err(corruption(
            material,
            "length exceeds the decompression limit",
        ));
    }
    Ok(length)
}

fn corruption(material: &str, reason: impl std::fmt::Display) -> AppError {
    AppError::operational(format!(
        "error: cache corruption in hunk {material}: {reason}; delete .gitscry and retry"
    ))
}

fn storage_error(operation: &str, error: impl std::fmt::Display) -> AppError {
    AppError::operational(format!("error: {operation}: {error}"))
}

fn put_varint(mut value: u32, output: &mut Vec<u8>) {
    while value >= 0x80 {
        output.push(value as u8 | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

fn read_varint(bytes: &[u8], cursor: &mut usize, material: &str) -> Result<u32, AppError> {
    let mut value = 0u32;
    for shift in (0..35).step_by(7) {
        let byte = *bytes
            .get(*cursor)
            .ok_or_else(|| corruption(material, "truncated token"))?;
        *cursor += 1;
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(corruption(material, "invalid token"))
}

fn lines(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut start = 0;
    let mut end = 0;
    std::iter::from_fn(move || {
        while end < bytes.len() && bytes[end] != b'\n' {
            end += 1;
        }
        if end == bytes.len() {
            if start == end {
                return None;
            }
            let result = &bytes[start..end];
            start = end;
            return Some(result);
        }
        let result = &bytes[start..=end];
        end += 1;
        start = end;
        Some(result)
    })
}

struct PendingPayload {
    payload_id: i64,
    token_offset: usize,
    token_length: usize,
    text_length: usize,
}

struct PendingHunk {
    change_id: i64,
    ordinal: i64,
    old_start: i64,
    old_lines: i64,
    new_start: i64,
    new_lines: i64,
    payload_id: i64,
}

pub(crate) struct HunkWriter {
    line_ids: HashMap<Box<[u8]>, u32>,
    next_line_id: u32,
    line_block_id: i64,
    line_block_first_id: u32,
    line_block_line_count: u32,
    line_block: Vec<u8>,
    token_block_id: i64,
    token_block: Vec<u8>,
    pending_payloads: Vec<PendingPayload>,
    pending_hunks: Vec<PendingHunk>,
    next_payload_id: i64,
}

impl HunkWriter {
    pub(crate) fn new(connection: &Connection) -> Result<Self, AppError> {
        let mut line_ids = HashMap::new();
        let mut next_line_id = 0u32;
        let mut statement = connection
            .prepare(
                "SELECT first_line_id, line_count, text, text_length
                 FROM hunk_line_blocks ORDER BY first_line_id",
            )
            .map_err(|error| storage_error("preparing hunk line dictionary", error))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(|error| storage_error("reading hunk line dictionary", error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage_error("reading hunk line dictionary", error))?;
        drop(statement);
        for (first_line_id, line_count, compressed, length) in rows {
            let first_line_id = u32::try_from(first_line_id)
                .map_err(|_| corruption("line dictionary", "invalid first line ID"))?;
            let line_count = u32::try_from(line_count)
                .map_err(|_| corruption("line dictionary", "invalid line count"))?;
            let bytes = decode(&compressed, length, "line dictionary")?;
            let mut cursor = 0usize;
            for offset in 0..line_count {
                let length = bytes
                    .get(cursor..cursor + 4)
                    .and_then(|bytes| bytes.try_into().ok())
                    .map(u32::from_le_bytes)
                    .ok_or_else(|| corruption("line dictionary", "truncated line"))?;
                cursor += 4;
                let length = usize::try_from(length)
                    .map_err(|_| corruption("line dictionary", "invalid line length"))?;
                let end = cursor
                    .checked_add(length)
                    .ok_or_else(|| corruption("line dictionary", "line length overflow"))?;
                let line = bytes
                    .get(cursor..end)
                    .ok_or_else(|| corruption("line dictionary", "truncated line"))?;
                line_ids.insert(Box::<[u8]>::from(line), first_line_id + offset);
                cursor = end;
            }
            if cursor != bytes.len() {
                return Err(corruption("line dictionary", "unexpected trailing bytes"));
            }
            next_line_id = next_line_id.max(first_line_id.saturating_add(line_count));
        }

        let line_block_id = next_id(connection, "hunk_line_blocks", "block_id")?;
        let token_block_id = next_id(connection, "hunk_token_blocks", "block_id")?;
        let next_payload_id = next_id(connection, "hunk_payloads", "payload_id")?;
        Ok(Self {
            line_ids,
            next_line_id,
            line_block_id,
            line_block_first_id: 0,
            line_block_line_count: 0,
            line_block: Vec::new(),
            token_block_id,
            token_block: Vec::new(),
            pending_payloads: Vec::new(),
            pending_hunks: Vec::new(),
            next_payload_id,
        })
    }

    pub(crate) fn write(
        &mut self,
        transaction: &Transaction<'_>,
        change_id: i64,
        hunk: &Hunk,
    ) -> Result<(), AppError> {
        let payload_id = self.next_payload_id;
        self.next_payload_id += 1;
        let mut tokens = Vec::with_capacity(hunk.text.len() / 2 + 4);
        let text_length = u32::try_from(hunk.text.len())
            .map_err(|_| storage_error("encoding text hunk", "payload exceeds 4 GiB"))?;
        tokens.extend_from_slice(&text_length.to_le_bytes());
        for line in lines(&hunk.text) {
            let line_id = if let Some(line_id) = self.line_ids.get(line) {
                *line_id
            } else {
                let line_length = u32::try_from(line.len())
                    .map_err(|_| storage_error("encoding line dictionary", "line exceeds 4 GiB"))?;
                let entry_length = line.len().checked_add(4).ok_or_else(|| {
                    storage_error("encoding line dictionary", "line is too large")
                })?;
                if !self.line_block.is_empty()
                    && entry_length > BLOCK_BYTES.saturating_sub(self.line_block.len())
                {
                    self.flush_line_block(transaction)?;
                }
                let line_id = self.next_line_id;
                self.next_line_id = self
                    .next_line_id
                    .checked_add(1)
                    .ok_or_else(|| storage_error("encoding line dictionary", "line ID overflow"))?;
                self.line_ids.insert(Box::<[u8]>::from(line), line_id);
                if self.line_block.is_empty() {
                    self.line_block_first_id = line_id;
                }
                self.line_block
                    .extend_from_slice(&line_length.to_le_bytes());
                self.line_block.extend_from_slice(line);
                self.line_block_line_count += 1;
                if self.line_block.len() >= BLOCK_BYTES {
                    self.flush_line_block(transaction)?;
                }
                line_id
            };
            put_varint(line_id, &mut tokens);
        }
        if !self.token_block.is_empty()
            && tokens.len() > BLOCK_BYTES.saturating_sub(self.token_block.len())
        {
            self.flush_token_block(transaction)?;
        }
        let token_offset = self.token_block.len();
        self.token_block.extend_from_slice(&tokens);
        self.pending_payloads.push(PendingPayload {
            payload_id,
            token_offset,
            token_length: tokens.len(),
            text_length: hunk.text.len(),
        });
        self.pending_hunks.push(PendingHunk {
            change_id,
            ordinal: hunk.ordinal,
            old_start: hunk.old_start,
            old_lines: hunk.old_lines,
            new_start: hunk.new_start,
            new_lines: hunk.new_lines,
            payload_id,
        });
        if self.token_block.len() >= BLOCK_BYTES {
            self.flush_token_block(transaction)?;
        }
        Ok(())
    }

    pub(crate) fn finish(&mut self, transaction: &Transaction<'_>) -> Result<(), AppError> {
        if !self.line_block.is_empty() {
            self.flush_line_block(transaction)?;
        }
        if !self.token_block.is_empty() {
            self.flush_token_block(transaction)?;
        }
        Ok(())
    }

    fn flush_line_block(&mut self, transaction: &Transaction<'_>) -> Result<(), AppError> {
        let (text, text_length) = encode(&self.line_block);
        transaction
            .execute(
                "INSERT INTO hunk_line_blocks(block_id, first_line_id, line_count, text, text_length)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    self.line_block_id,
                    self.line_block_first_id,
                    self.line_block_line_count,
                    text,
                    text_length,
                ],
            )
            .map_err(|error| storage_error("writing hunk line dictionary", error))?;
        self.line_block_id += 1;
        reset_block(&mut self.line_block);
        self.line_block_line_count = 0;
        Ok(())
    }

    fn flush_token_block(&mut self, transaction: &Transaction<'_>) -> Result<(), AppError> {
        let (text, text_length) = encode(&self.token_block);
        transaction
            .execute(
                "INSERT INTO hunk_token_blocks(block_id, text, text_length) VALUES (?1, ?2, ?3)",
                params![self.token_block_id, text, text_length],
            )
            .map_err(|error| storage_error("writing hunk token block", error))?;
        for payload in &self.pending_payloads {
            transaction
                .execute(
                    "INSERT INTO hunk_payloads(payload_id, token_block_id, token_offset, token_length, text_length)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        payload.payload_id,
                        self.token_block_id,
                        payload.token_offset,
                        payload.token_length,
                        payload.text_length,
                    ],
                )
                .map_err(|error| storage_error("writing hunk payload index", error))?;
        }
        for hunk in &self.pending_hunks {
            transaction
                .execute(
                    "INSERT INTO hunks(change_id, ordinal, old_start, old_lines, new_start, new_lines, payload_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        hunk.change_id,
                        hunk.ordinal,
                        hunk.old_start,
                        hunk.old_lines,
                        hunk.new_start,
                        hunk.new_lines,
                        hunk.payload_id,
                    ],
                )
                .map_err(|error| storage_error("writing hunk metadata", error))?;
        }
        self.pending_payloads.clear();
        self.pending_hunks.clear();
        self.token_block_id += 1;
        reset_block(&mut self.token_block);
        Ok(())
    }
}

fn reset_block(buffer: &mut Vec<u8>) {
    if buffer.capacity() > BLOCK_BYTES {
        *buffer = Vec::new();
    } else {
        buffer.clear();
    }
}

fn next_id(connection: &Connection, table: &str, column: &str) -> Result<i64, AppError> {
    let query = format!("SELECT COALESCE(MAX({column}), 0) + 1 FROM {table}");
    connection
        .query_row(&query, [], |row| row.get(0))
        .map_err(|error| storage_error("allocating cache ID", error))
}

struct LineBlock {
    block_id: i64,
    first_line_id: u32,
    line_count: u32,
    text_length: i64,
}

pub(crate) struct HunkReader<'a> {
    connection: &'a Connection,
    line_blocks: Vec<LineBlock>,
    line_cache: HashMap<i64, Vec<Vec<u8>>>,
    token_cache: HashMap<i64, Vec<u8>>,
}

impl<'a> HunkReader<'a> {
    pub(crate) fn new(connection: &'a Connection) -> Result<Self, AppError> {
        let mut statement = connection
            .prepare(
                "SELECT block_id, first_line_id, line_count, text_length
                 FROM hunk_line_blocks ORDER BY first_line_id",
            )
            .map_err(|error| storage_error("preparing hunk line dictionary", error))?;
        let line_blocks = statement
            .query_map([], |row| {
                Ok(LineBlock {
                    block_id: row.get(0)?,
                    first_line_id: u32::try_from(row.get::<_, i64>(1)?).unwrap_or(u32::MAX),
                    line_count: u32::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
                    text_length: row.get(3)?,
                })
            })
            .map_err(|error| storage_error("reading hunk line dictionary", error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage_error("reading hunk line dictionary", error))?;
        Ok(Self {
            connection,
            line_blocks,
            line_cache: HashMap::new(),
            token_cache: HashMap::new(),
        })
    }

    pub(crate) fn decode_payload(
        &mut self,
        payload_id: i64,
        material: &str,
    ) -> Result<Vec<u8>, AppError> {
        let (token_block_id, token_offset, token_length, text_length): (i64, i64, i64, i64) = self
            .connection
            .query_row(
                "SELECT token_block_id, token_offset, token_length, text_length
                 FROM hunk_payloads WHERE payload_id = ?1",
                [payload_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => {
                    corruption(material, "missing hunk payload index")
                }
                error => storage_error("reading hunk payload index", error),
            })?;
        let text_length = checked_length(text_length, material)?;
        let token_offset = usize::try_from(token_offset)
            .map_err(|_| corruption(material, "invalid token offset"))?;
        let token_length = usize::try_from(token_length)
            .map_err(|_| corruption(material, "invalid token length"))?;
        let token_end = token_offset
            .checked_add(token_length)
            .ok_or_else(|| corruption(material, "token range overflow"))?;
        let tokens = self
            .token_block(token_block_id, material)?
            .get(token_offset..token_end)
            .ok_or_else(|| corruption(material, "truncated token block"))?
            .to_vec();
        let stored_length = tokens
            .get(..4)
            .and_then(|bytes| bytes.try_into().ok())
            .map(u32::from_le_bytes)
            .ok_or_else(|| corruption(material, "truncated token payload"))?;
        if usize::try_from(stored_length).ok() != Some(text_length) {
            return Err(corruption(material, "text length metadata disagrees"));
        }
        let mut cursor = 4usize;
        let mut output = Vec::with_capacity(text_length);
        while output.len() < text_length {
            let line_id = read_varint(&tokens, &mut cursor, material)?;
            let line = self.line(line_id, material)?;
            output.extend_from_slice(&line);
            if output.len() > text_length {
                return Err(corruption(material, "decoded text exceeds its length"));
            }
        }
        if cursor != tokens.len() || output.len() != text_length {
            return Err(corruption(
                material,
                "decoded text does not consume its token payload",
            ));
        }
        Ok(output)
    }

    fn token_block(&mut self, block_id: i64, material: &str) -> Result<&[u8], AppError> {
        if !self.token_cache.contains_key(&block_id) {
            let (compressed, length): (Vec<u8>, i64) = self
                .connection
                .query_row(
                    "SELECT text, text_length FROM hunk_token_blocks WHERE block_id = ?1",
                    [block_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|error| match error {
                    rusqlite::Error::QueryReturnedNoRows => {
                        corruption(material, "missing hunk token block")
                    }
                    error => storage_error("reading hunk token block", error),
                })?;
            let decoded = decode(&compressed, length, material)?;
            self.token_cache.insert(block_id, decoded);
        }
        Ok(self
            .token_cache
            .get(&block_id)
            .expect("inserted token block"))
    }

    fn line(&mut self, line_id: u32, material: &str) -> Result<Vec<u8>, AppError> {
        let Some(block_index) = self
            .line_blocks
            .iter()
            .rposition(|block| block.first_line_id <= line_id)
        else {
            return Err(corruption(material, "line ID is outside the dictionary"));
        };
        let block = &self.line_blocks[block_index];
        let offset = line_id - block.first_line_id;
        if offset >= block.line_count {
            return Err(corruption(
                material,
                "line ID is outside its dictionary block",
            ));
        }
        let block_id = block.block_id;
        if !self.line_cache.contains_key(&block_id) {
            let (compressed, length): (Vec<u8>, i64) = self
                .connection
                .query_row(
                    "SELECT text, text_length FROM hunk_line_blocks WHERE block_id = ?1",
                    [block_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|error| match error {
                    rusqlite::Error::QueryReturnedNoRows => {
                        corruption(material, "missing hunk line dictionary block")
                    }
                    error => storage_error("reading hunk line dictionary", error),
                })?;
            if length != block.text_length {
                return Err(corruption(
                    material,
                    "line dictionary length metadata disagrees",
                ));
            }
            let bytes = decode(&compressed, length, material)?;
            let mut cursor = 0usize;
            let mut lines = Vec::with_capacity(block.line_count as usize);
            for _ in 0..block.line_count {
                let length = bytes
                    .get(cursor..cursor + 4)
                    .and_then(|bytes| bytes.try_into().ok())
                    .map(u32::from_le_bytes)
                    .ok_or_else(|| corruption(material, "truncated line dictionary"))?;
                cursor += 4;
                let length = usize::try_from(length)
                    .map_err(|_| corruption(material, "invalid line dictionary length"))?;
                let end = cursor
                    .checked_add(length)
                    .ok_or_else(|| corruption(material, "line dictionary overflow"))?;
                lines.push(
                    bytes
                        .get(cursor..end)
                        .ok_or_else(|| corruption(material, "truncated line dictionary"))?
                        .to_vec(),
                );
                cursor = end;
            }
            if cursor != bytes.len() {
                return Err(corruption(material, "unexpected line dictionary bytes"));
            }
            self.line_cache.insert(block_id, lines);
        }
        Ok(self.line_cache[&block_id][offset as usize].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};

    #[test]
    fn compressed_payloads_round_trip_bytes() {
        for bytes in [
            b"".as_slice(),
            b"not utf-8: \xff\xfe",
            vec![b'x'; 1_000_000].as_slice(),
        ] {
            let (compressed, length) = encode(bytes);
            assert_eq!(decode(&compressed, length, "commit/change").unwrap(), bytes);
        }
    }

    #[test]
    fn malformed_payload_is_rejected() {
        let (compressed, length) = encode(b"valid");
        let error = decode(&compressed, length + 1, "commit/change").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("cache corruption in hunk commit/change")
        );
    }

    #[test]
    fn hunk_archive_round_trips_and_reports_corruption() {
        use super::{HunkReader, HunkWriter};
        use crate::git::Hunk;
        use rusqlite::{Connection, params};

        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(super::super::schema::SCHEMA)
            .unwrap();
        connection
            .execute(
                "INSERT INTO commits(position, oid, message, message_length, commit_time)
                 VALUES (0, ?1, ?2, 0, 0)",
                params!["oid", Vec::<u8>::new()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO changes(change_id, commit_id, ordinal, status, old_mode, new_mode)
                 VALUES (1, 1, 0, 'M', '100644', '100644')",
                [],
            )
            .unwrap();

        let mut text = b"first\nnot utf-8: \xff\n".to_vec();
        text.extend(std::iter::repeat_n(b'x', 1_000_000));
        let hunk = Hunk {
            commit_oid: "oid".to_owned(),
            change_ordinal: 0,
            ordinal: 0,
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
            text: text.clone(),
        };
        let transaction = connection.transaction().unwrap();
        let mut writer = HunkWriter::new(&transaction).unwrap();
        writer.write(&transaction, 1, &hunk).unwrap();
        writer.finish(&transaction).unwrap();
        transaction.commit().unwrap();

        let payload_id: i64 = connection
            .query_row("SELECT payload_id FROM hunks", [], |row| row.get(0))
            .unwrap();
        let mut reader = HunkReader::new(&connection).unwrap();
        assert_eq!(
            reader
                .decode_payload(payload_id, "oid/change 0/hunk 0")
                .unwrap(),
            text
        );

        connection
            .execute("UPDATE hunk_token_blocks SET text = ?1", params![vec![0u8]])
            .unwrap();
        let mut reader = HunkReader::new(&connection).unwrap();
        let error = reader
            .decode_payload(payload_id, "oid/change 0/hunk 0")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("cache corruption in hunk oid/change 0/hunk 0")
        );
    }

    #[test]
    fn failed_hunk_flush_rolls_back_token_and_payload_rows() {
        use super::{BLOCK_BYTES, HunkWriter};
        use crate::git::Hunk;
        use rusqlite::{Connection, params};

        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(super::super::schema::SCHEMA)
            .unwrap();
        connection
            .execute(
                "INSERT INTO commits(position, oid, message, message_length, commit_time)
                 VALUES (0, ?1, ?2, 0, 0)",
                params!["oid", Vec::<u8>::new()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO changes(change_id, commit_id, ordinal, status, old_mode, new_mode)
                 VALUES (1, 1, 0, 'M', '100644', '100644')",
                [],
            )
            .unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER reject_hunk_metadata BEFORE INSERT ON hunks
                 BEGIN SELECT RAISE(ABORT, 'forced hunk metadata failure'); END;",
            )
            .unwrap();

        let mut text = b"@@ -1 +1 @@\n".to_vec();
        for _ in 0..100 {
            text.extend_from_slice(b"+same\n");
        }
        assert!(text.len() > BLOCK_BYTES);
        let hunk = Hunk {
            commit_oid: "oid".to_owned(),
            change_ordinal: 0,
            ordinal: 0,
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
            text,
        };

        let transaction = connection.transaction().unwrap();
        let mut writer = HunkWriter::new(&transaction).unwrap();
        let error = writer.write(&transaction, 1, &hunk).unwrap_err();
        assert!(error.to_string().contains("writing hunk metadata"));
        drop(writer);
        drop(transaction);

        for table in ["hunk_token_blocks", "hunk_payloads", "hunks"] {
            let count: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "{table} should roll back with the failed flush");
        }
    }
    #[test]
    fn writer_splits_both_blocks_without_splitting_payloads() {
        use super::{BLOCK_BYTES, HunkReader, HunkWriter};
        use crate::git::Hunk;
        use rusqlite::{Connection, params};

        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(super::super::schema::SCHEMA)
            .unwrap();
        connection
            .execute(
                "INSERT INTO commits(position, oid, message, message_length, commit_time)
                 VALUES (0, ?1, ?2, 0, 0)",
                params!["oid", Vec::<u8>::new()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO changes(change_id, commit_id, ordinal, status, old_mode, new_mode)
                 VALUES (1, 1, 0, 'M', '100644', '100644')",
                [],
            )
            .unwrap();

        let mut first = b"@@ -1,3 +1,3 @@\n".to_vec();
        for index in 0..3 {
            first.extend_from_slice(format!("+unique-{index}-padding\n").as_bytes());
        }
        let repeated_hunk = || {
            let mut text = b"@@ -1 +1 @@\n".to_vec();
            for _ in 0..50 {
                text.extend_from_slice(b"+same\n");
            }
            text
        };
        let mut oversized_tokens = b"@@ -1 +1 @@\n".to_vec();
        for _ in 0..100 {
            oversized_tokens.extend_from_slice(b"+same\n");
        }
        let mut oversized_line = b"@@ -1 +1 @@\n+".to_vec();
        oversized_line.extend(std::iter::repeat_n(b'x', BLOCK_BYTES + 1));
        oversized_line.push(b'\n');
        let final_partial = b"@@ -1 +1 @@\n+tail\n".to_vec();
        let hunk_texts = [
            first,
            repeated_hunk(),
            repeated_hunk(),
            oversized_tokens,
            oversized_line,
            final_partial,
        ];

        let transaction = connection.transaction().unwrap();
        let mut writer = HunkWriter::new(&transaction).unwrap();
        for (ordinal, text) in hunk_texts.iter().enumerate() {
            let hunk = Hunk {
                commit_oid: "oid".to_owned(),
                change_ordinal: 0,
                ordinal: ordinal as i64,
                old_start: 1,
                old_lines: 1,
                new_start: 1,
                new_lines: 1,
                text: text.clone(),
            };
            writer.write(&transaction, 1, &hunk).unwrap();
            if ordinal == 3 {
                assert_eq!(writer.token_block.capacity(), 0);
            }
            if ordinal == 4 {
                assert_eq!(writer.line_block.capacity(), 0);
            }
        }
        let flushed_hunks: i64 = transaction
            .query_row("SELECT COUNT(*) FROM hunks", [], |row| row.get(0))
            .unwrap();
        let flushed_payloads: i64 = transaction
            .query_row("SELECT COUNT(*) FROM hunk_payloads", [], |row| row.get(0))
            .unwrap();
        assert_eq!(flushed_hunks, 4);
        assert_eq!(flushed_payloads, 4);
        writer.finish(&transaction).unwrap();
        transaction.commit().unwrap();

        let line_block_lengths: Vec<i64> = connection
            .prepare("SELECT text_length FROM hunk_line_blocks ORDER BY block_id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(line_block_lengths.len() > 1);
        assert_eq!(
            line_block_lengths
                .iter()
                .filter(|length| **length > BLOCK_BYTES as i64)
                .count(),
            1
        );

        let token_block_lengths: Vec<i64> = connection
            .prepare("SELECT text_length FROM hunk_token_blocks ORDER BY block_id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(token_block_lengths.len() > 1);
        assert_eq!(
            token_block_lengths
                .iter()
                .filter(|length| **length > BLOCK_BYTES as i64)
                .count(),
            1
        );

        let payload_rows: Vec<(i64, i64, i64, i64, i64)> = connection
            .prepare(
                "SELECT h.ordinal, p.token_block_id, p.token_offset, p.token_length, b.text_length
                 FROM hunks AS h
                 JOIN hunk_payloads AS p ON p.payload_id = h.payload_id
                 JOIN hunk_token_blocks AS b ON b.block_id = p.token_block_id
                 ORDER BY h.ordinal",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(payload_rows.len(), hunk_texts.len());
        assert_eq!(payload_rows[0].1, payload_rows[1].1);
        assert_ne!(payload_rows[1].1, payload_rows[2].1);
        assert_ne!(payload_rows[2].1, payload_rows[3].1);
        assert_eq!(payload_rows[3].2, 0);
        assert!(payload_rows[3].3 > BLOCK_BYTES as i64);
        assert_eq!(payload_rows[3].3, payload_rows[3].4);
        assert_eq!(payload_rows[4].1, payload_rows[5].1);

        let mut reader = HunkReader::new(&connection).unwrap();
        for (ordinal, expected) in hunk_texts.iter().enumerate() {
            let payload_id: i64 = connection
                .query_row(
                    "SELECT payload_id FROM hunks WHERE ordinal = ?1",
                    [ordinal as i64],
                    |row| row.get(0),
                )
                .unwrap();
            let decoded = reader
                .decode_payload(payload_id, &format!("oid/hunk {ordinal}"))
                .unwrap();
            assert_eq!(&decoded, expected);
        }
    }
}
