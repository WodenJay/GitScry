use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value};

use crate::app::AppError;

use super::super::Step;
use super::super::search_error;

/// A lexical candidate: one cache row with the paths it changed.
pub(super) struct Stored {
    pub(super) position: i64,
    pub(super) oid: String,
    pub(super) commit_time: i64,
    pub(super) subject: String,
    pub(super) body: String,
    pub(super) paths: Vec<Vec<u8>>,
    pub(super) bm25: f64,
}

pub(in crate::analysis) fn match_count(
    connection: &Connection,
    match_query: &str,
) -> Result<usize, AppError> {
    let count = connection
        .query_row(
            "SELECT COUNT(*) FROM search_fts WHERE search_fts MATCH ?1",
            [match_query],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| search_error("counting search matches", error))?;
    usize::try_from(count)
        .map_err(|_| search_error("counting search matches", "count exceeded platform limits"))
}

/// The strongest lexical candidates, each carrying the paths it changed.
pub(in crate::analysis) fn candidates(
    connection: &Connection,
    match_query: &str,
    limit: i64,
) -> Result<Vec<Stored>, AppError> {
    let mut statement = connection
        .prepare(
            "SELECT c.oid, c.commit_time, c.message, bm25(search_fts, 10.0, 3.0, 2.0), c.rowid
             FROM search_fts
             JOIN search_documents AS d ON d.rowid = search_fts.rowid
             JOIN commits AS c ON c.oid = d.commit_oid
             WHERE search_fts MATCH ?1
             ORDER BY bm25(search_fts, 10.0, 3.0, 2.0), c.commit_time DESC, c.oid ASC
             LIMIT ?2",
        )
        .map_err(|error| search_error("preparing search", error))?;
    let rows = statement
        .query_map(params![match_query, limit], |row| {
            let message: Vec<u8> = row.get(2)?;
            let (subject, body) = super::text::message_parts(&message);
            Ok(Stored {
                oid: row.get(0)?,
                commit_time: row.get(1)?,
                subject,
                body,
                paths: Vec::new(),
                bm25: row.get(3)?,
                position: row.get(4)?,
            })
        })
        .map_err(|error| search_error("running search", error))?;
    let mut candidates = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| search_error("reading search results", error))?;
    for candidate in &mut candidates {
        candidate.paths = changed_paths(connection, &candidate.oid)?;
    }
    Ok(candidates)
}

pub(super) fn changed_paths(connection: &Connection, oid: &str) -> Result<Vec<Vec<u8>>, AppError> {
    let mut statement = connection
        .prepare("SELECT old_path, new_path FROM changes WHERE commit_oid = ?1 ORDER BY ordinal")
        .map_err(|error| search_error("preparing changed paths", error))?;
    let rows = statement
        .query_map([oid], |row| {
            Ok((
                row.get::<_, Option<Vec<u8>>>(0)?,
                row.get::<_, Option<Vec<u8>>>(1)?,
            ))
        })
        .map_err(|error| search_error("reading changed paths", error))?;
    let mut paths = Vec::new();
    for row in rows {
        let (old_path, new_path) =
            row.map_err(|error| search_error("reading changed paths", error))?;
        for path in [old_path, new_path].into_iter().flatten() {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    Ok(paths)
}

/// The moves a commit made, in change order and deduplicated.
pub(in crate::analysis) fn steps(
    connection: &Connection,
    oid: &str,
) -> Result<Vec<Step>, AppError> {
    let mut statement = connection
        .prepare(
            "SELECT status, old_path, new_path FROM changes WHERE commit_oid = ?1 ORDER BY ordinal",
        )
        .map_err(|error| search_error("preparing change shapes", error))?;
    let rows = statement
        .query_map([oid], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<Vec<u8>>>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
            ))
        })
        .map_err(|error| search_error("reading change shapes", error))?;
    let mut steps: Vec<Step> = Vec::new();
    for row in rows {
        let (status, old_path, new_path) =
            row.map_err(|error| search_error("reading change shapes", error))?;
        if let Some(step) = Step::from_change(&status, old_path, new_path)
            && !steps.contains(&step)
        {
            steps.push(step);
        }
    }
    Ok(steps)
}

/// Subject and body of one cached commit.
pub(in crate::analysis) fn text(
    connection: &Connection,
    oid: &str,
) -> Result<Option<(String, String)>, AppError> {
    let mut statement = connection
        .prepare("SELECT message FROM commits WHERE oid = ?1")
        .map_err(|error| search_error("preparing commit lookup", error))?;
    let message = statement
        .query_row([oid], |row| row.get::<_, Vec<u8>>(0))
        .optional()
        .map_err(|error| search_error("reading commit lookup", error))?;
    Ok(message.map(|message| super::text::message_parts(&message)))
}

/// A cached commit: its object ID, message, and cache position.
///
/// The position orders commits the way the cache generation was built, which separates
/// commits a repository recorded in the same second.
pub(in crate::analysis) struct StoredCommit {
    pub(in crate::analysis) position: i64,
    pub(in crate::analysis) oid: String,
    pub(in crate::analysis) message: Vec<u8>,
}

/// Every cached commit, oldest first.
pub(in crate::analysis) fn history(connection: &Connection) -> Result<Vec<StoredCommit>, AppError> {
    let mut statement = connection
        .prepare("SELECT rowid, oid, message FROM commits ORDER BY commit_time, oid")
        .map_err(|error| search_error("preparing history scan", error))?;
    statement
        .query_map([], |row| {
            Ok(StoredCommit {
                position: row.get(0)?,
                oid: row.get(1)?,
                message: row.get(2)?,
            })
        })
        .map_err(|error| search_error("reading history scan", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| search_error("reading history scan", error))
}

/// Whether any commit strictly between two positions touched one of `paths`.
///
/// A revert only counts as undoing a candidate when that candidate was the last work on
/// the path; otherwise the revert belongs to some later, unrelated change.
pub(in crate::analysis) fn touch_between(
    connection: &Connection,
    from_position: i64,
    to_position: i64,
    paths: &[Vec<u8>],
) -> Result<bool, AppError> {
    if paths.is_empty() || from_position >= to_position {
        return Ok(false);
    }
    let placeholders = std::iter::repeat_n("?", paths.len())
        .collect::<Vec<_>>()
        .join(", ");
    let query = format!(
        "SELECT COUNT(*)
         FROM commits AS c
         JOIN changes AS ch ON ch.commit_oid = c.oid
         WHERE c.rowid > ?1 AND c.rowid < ?2
           AND (ch.old_path IN ({placeholders}) OR ch.new_path IN ({placeholders}))",
    );
    let values = [Value::Integer(from_position), Value::Integer(to_position)]
        .into_iter()
        .chain(paths.iter().map(|path| Value::Blob(path.clone())))
        .chain(paths.iter().map(|path| Value::Blob(path.clone())));
    let count: i64 = connection
        .query_row(&query, params_from_iter(values), |row| row.get(0))
        .map_err(|error| search_error("checking intervening history", error))?;
    Ok(count > 0)
}

/// The first later commit that corrects work on the abandoned paths, if history records one.
///
/// Ordering is by cache insertion order rather than timestamp, because a repository can
/// record a revert and its follow-up in the same second.
pub(in crate::analysis) fn corrective_follow_up(
    connection: &Connection,
    revert_oid: &str,
    paths: &[Vec<u8>],
) -> Result<Option<(String, String)>, AppError> {
    // The placeholder list is interpolated twice, once per IN list.
    let placeholders = std::iter::repeat_n("?", paths.len())
        .collect::<Vec<_>>()
        .join(", ");
    let query = format!(
        "SELECT c.oid, c.message
         FROM commits AS c
         JOIN changes AS ch ON ch.commit_oid = c.oid
         WHERE c.rowid > (SELECT rowid FROM commits WHERE oid = ?1)
           AND c.oid NOT IN (SELECT oid FROM commits WHERE oid = ?1)
           AND (ch.old_path IN ({placeholders}) OR ch.new_path IN ({placeholders}))
         GROUP BY c.oid
         ORDER BY c.rowid ASC",
    );
    let values = std::iter::once(Value::Text(revert_oid.to_owned()))
        .chain(paths.iter().map(|path| Value::Blob(path.clone())))
        .chain(paths.iter().map(|path| Value::Blob(path.clone())));
    let mut statement = connection
        .prepare(&query)
        .map_err(|error| search_error("preparing corrective follow-up", error))?;
    let rows = statement
        .query_map(params_from_iter(values), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(|error| search_error("reading corrective follow-up", error))?;
    for row in rows {
        let (oid, message) =
            row.map_err(|error| search_error("reading corrective follow-up", error))?;
        let (subject, _) = super::text::message_parts(&message);
        // Only a commit that reads as a correction counts; a later incidental touch of the
        // same path is not evidence about how the abandoned approach moved on.
        if super::super::provenance::is_corrective_subject(&subject) {
            return Ok(Some((oid, subject)));
        }
    }
    Ok(None)
}
