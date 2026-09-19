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
    oid TEXT PRIMARY KEY,
    message BLOB NOT NULL,
    commit_time INTEGER NOT NULL
) STRICT;
CREATE TABLE search_documents (
    rowid INTEGER PRIMARY KEY,
    commit_oid TEXT NOT NULL UNIQUE REFERENCES commits(oid),
    subject TEXT NOT NULL,
    body TEXT NOT NULL,
    paths TEXT NOT NULL
) STRICT;
CREATE VIRTUAL TABLE search_fts USING fts5(
    subject,
    body,
    paths,
    content = 'search_documents',
    content_rowid = 'rowid',
    tokenize = 'unicode61',
    detail = 'full'
);
CREATE TABLE commit_parents (
    commit_oid TEXT NOT NULL REFERENCES commits(oid),
    position INTEGER NOT NULL,
    parent_oid TEXT NOT NULL,
    PRIMARY KEY (commit_oid, position)
) STRICT;
CREATE TABLE changes (
    commit_oid TEXT NOT NULL REFERENCES commits(oid),
    ordinal INTEGER NOT NULL,
    status TEXT NOT NULL,
    old_path BLOB,
    new_path BLOB,
    old_blob TEXT,
    new_blob TEXT,
    old_mode TEXT NOT NULL,
    new_mode TEXT NOT NULL,
    PRIMARY KEY (commit_oid, ordinal)
) STRICT;
CREATE TABLE hunks (
    commit_oid TEXT NOT NULL,
    change_ordinal INTEGER NOT NULL,
    ordinal INTEGER NOT NULL,
    old_start INTEGER NOT NULL,
    old_lines INTEGER NOT NULL,
    new_start INTEGER NOT NULL,
    new_lines INTEGER NOT NULL,
    text BLOB NOT NULL,
    PRIMARY KEY (commit_oid, change_ordinal, ordinal),
    FOREIGN KEY (commit_oid, change_ordinal)
        REFERENCES changes(commit_oid, ordinal)
) STRICT;
"#;
