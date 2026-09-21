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
CREATE TABLE hunks (
    hunk_id INTEGER PRIMARY KEY,
    change_id INTEGER NOT NULL REFERENCES changes(change_id),
    ordinal INTEGER NOT NULL,
    old_start INTEGER NOT NULL,
    old_lines INTEGER NOT NULL,
    new_start INTEGER NOT NULL,
    new_lines INTEGER NOT NULL,
    text BLOB NOT NULL,
    text_length INTEGER NOT NULL CHECK (text_length >= 0),
    UNIQUE (change_id, ordinal)
) STRICT;
"#;
