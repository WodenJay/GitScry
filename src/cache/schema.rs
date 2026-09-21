pub(super) const SCHEMA: &str = r#"
PRAGMA journal_mode = DELETE;
PRAGMA synchronous = FULL;
PRAGMA foreign_keys = ON;
CREATE TABLE metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;
CREATE TABLE shallow_boundaries (
    oid TEXT PRIMARY KEY
) STRICT;
CREATE TABLE missing_objects (
    oid TEXT PRIMARY KEY
) STRICT;
CREATE TABLE commits (
    commit_id INTEGER PRIMARY KEY,
    position INTEGER NOT NULL UNIQUE,
    oid TEXT NOT NULL UNIQUE,
    message BLOB NOT NULL,
    message_length INTEGER NOT NULL CHECK (message_length >= 0),
    commit_time INTEGER NOT NULL
) STRICT;
CREATE VIRTUAL TABLE search_fts USING fts5(
    subject,
    body,
    paths,
    content = '',
    contentless_delete = 1,
    tokenize = 'unicode61',
    detail = 'full',
    columnsize = 1
);
CREATE TABLE commit_parents (
    commit_id INTEGER NOT NULL REFERENCES commits(commit_id),
    position INTEGER NOT NULL,
    parent_id INTEGER REFERENCES commits(commit_id),
    external_oid TEXT,
    PRIMARY KEY (commit_id, position),
    CHECK ((parent_id IS NULL) != (external_oid IS NULL))
) STRICT;
CREATE TABLE changes (
    change_id INTEGER PRIMARY KEY,
    commit_id INTEGER NOT NULL REFERENCES commits(commit_id),
    ordinal INTEGER NOT NULL,
    status TEXT NOT NULL,
    old_path BLOB,
    new_path BLOB,
    old_blob TEXT,
    new_blob TEXT,
    old_mode TEXT NOT NULL,
    new_mode TEXT NOT NULL,
    UNIQUE (commit_id, ordinal)
) STRICT;
CREATE TABLE commit_paths (
    commit_id INTEGER NOT NULL REFERENCES commits(commit_id),
    path_key TEXT NOT NULL,
    path_basename TEXT NOT NULL CHECK (path_basename <> ''),
    raw_path BLOB NOT NULL,
    path_order INTEGER NOT NULL CHECK (path_order >= 0),
    PRIMARY KEY (commit_id, path_key)
) STRICT;
CREATE INDEX commit_paths_by_key ON commit_paths(path_key, commit_id);
CREATE INDEX commit_paths_by_basename ON commit_paths(path_basename, commit_id);
CREATE INDEX commit_paths_by_commit ON commit_paths(commit_id, path_order);
CREATE TABLE commit_path_counts (
    commit_id INTEGER PRIMARY KEY REFERENCES commits(commit_id),
    path_count INTEGER NOT NULL CHECK (path_count >= 0)
 ) STRICT;
CREATE TABLE hunk_line_blocks (
    block_id INTEGER PRIMARY KEY,
    first_line_id INTEGER NOT NULL,
    line_count INTEGER NOT NULL CHECK (line_count > 0),
    text BLOB NOT NULL,
    text_length INTEGER NOT NULL CHECK (text_length >= 0)
) STRICT;
CREATE TABLE hunk_token_blocks (
    block_id INTEGER PRIMARY KEY,
    text BLOB NOT NULL,
    text_length INTEGER NOT NULL CHECK (text_length >= 0)
) STRICT;
CREATE TABLE hunk_payloads (
    payload_id INTEGER PRIMARY KEY,
    token_block_id INTEGER NOT NULL REFERENCES hunk_token_blocks(block_id),
    token_offset INTEGER NOT NULL CHECK (token_offset >= 0),
    token_length INTEGER NOT NULL CHECK (token_length > 0),
    text_length INTEGER NOT NULL CHECK (text_length >= 0)
) STRICT;
CREATE TABLE hunks (
    hunk_id INTEGER PRIMARY KEY,
    change_id INTEGER NOT NULL REFERENCES changes(change_id),
    ordinal INTEGER NOT NULL,
    old_start INTEGER NOT NULL,
    old_lines INTEGER NOT NULL,
    new_start INTEGER NOT NULL,
    new_lines INTEGER NOT NULL,
    payload_id INTEGER NOT NULL REFERENCES hunk_payloads(payload_id),
    UNIQUE (change_id, ordinal)
) STRICT;
"#;
