//! Planning, validating and safely publishing a pinned cache generation.

use std::{
    collections::HashSet,
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params_from_iter};

use crate::{
    app::{AppError, IndexStage},
    git::{HistoryTarget, Repository, Snapshot},
};

use super::{SCHEMA_VERSION, SharedLock, cache_error, write};

struct CacheState {
    tip: String,
    object_format: String,
    shallow_boundaries: Vec<String>,
    missing_objects: Vec<String>,
    referenced_objects: Vec<String>,
    commits: Vec<String>,
    commit_count: usize,
    semantic_enabled: bool,
}

enum Inspection {
    Missing,
    Stale { semantic_enabled: bool },
    Damaged { semantic_enabled: bool },
    Ready(CacheState),
}

pub(crate) struct PreparedCache {
    pub(crate) progress: Vec<String>,
    pub(crate) current_head_commit_count: usize,
    pub(crate) semantic_enabled: bool,
    _lock: SharedLock,
}

impl PreparedCache {
    pub(crate) fn release(self) -> (Vec<String>, usize, bool) {
        let Self {
            progress,
            current_head_commit_count,
            semantic_enabled,
            _lock,
        } = self;
        drop(_lock);
        (progress, current_head_commit_count, semantic_enabled)
    }
}

struct Expected {
    tip: String,
    object_format: String,
    shallow_boundaries: Vec<String>,
}

fn publication_matches_target(state: &CacheState, expected: &Expected) -> bool {
    // `tip` is the most recent publisher, not all cached HEADs.
    state.commits.contains(&expected.tip)
        && state.object_format == expected.object_format
        && state.shallow_boundaries == expected.shallow_boundaries
}

enum Plan {
    Fresh {
        state: CacheState,
        missing_objects: Vec<String>,
        current_head_commit_count: usize,
    },
    Incremental {
        state: CacheState,
        missing_objects: Vec<String>,
        changed_objects: Vec<String>,
        reachable_commits: Vec<String>,
    },
    Rebuild {
        damaged: bool,
        semantic_enabled: bool,
    },
}

pub(crate) fn prepare(
    repository: &Repository,
    report: &mut dyn FnMut(IndexStage),
) -> Result<PreparedCache, AppError> {
    let tip = repository.resolve_commit("HEAD")?;
    prepare_at(repository, tip, report)
}

fn prepare_at(
    repository: &Repository,
    tip: String,
    report: &mut dyn FnMut(IndexStage),
) -> Result<PreparedCache, AppError> {
    let expected = Expected {
        tip,
        object_format: repository.object_format()?,
        shallow_boundaries: repository.shallow_boundaries()?,
    };
    let mut progress = Vec::new();
    let shared = super::acquire_shared(&repository.common_dir, &mut progress)?;
    let plan = evaluate(repository, &expected, inspect(&repository.common_dir))?;
    if let Plan::Fresh {
        state,
        missing_objects,
        current_head_commit_count,
    } = plan
    {
        add_warnings(&mut progress, &expected, &missing_objects);
        return Ok(PreparedCache {
            progress,
            current_head_commit_count,
            semantic_enabled: state.semantic_enabled,
            _lock: shared,
        });
    }
    drop(shared);

    let exclusive = super::acquire_exclusive(&repository.common_dir, &mut progress)?;
    recover_previous(&repository.common_dir)?;
    let plan = evaluate(repository, &expected, inspect(&repository.common_dir))?;
    if let Plan::Fresh {
        state,
        missing_objects,
        current_head_commit_count,
    } = plan
    {
        drop(exclusive);
        let shared = super::acquire_shared(&repository.common_dir, &mut progress)?;
        add_warnings(&mut progress, &expected, &missing_objects);
        return Ok(PreparedCache {
            progress,
            current_head_commit_count,
            semantic_enabled: state.semantic_enabled,
            _lock: shared,
        });
    }

    let current_head_commit_count = match plan {
        Plan::Rebuild {
            damaged,
            semantic_enabled,
        } => {
            if damaged {
                preserve_damaged(&repository.common_dir)?;
            }
            report(IndexStage::ReadingCommits);
            let snapshot = repository.read_history_at(
                HistoryTarget {
                    tip: expected.tip.clone(),
                    object_format: expected.object_format.clone(),
                    shallow_boundaries: expected.shallow_boundaries.clone(),
                },
                report,
            )?;
            let count = snapshot.commits.len();
            report(IndexStage::WritingCache);
            publish(&repository.common_dir, &snapshot, semantic_enabled)?;
            count
        }
        Plan::Incremental {
            state,
            missing_objects,
            changed_objects,
            reachable_commits,
        } => {
            let current_head_commit_count = reachable_commits.len();
            report(IndexStage::ReadingCommits);
            let reachable = reachable_commits.iter().cloned().collect::<HashSet<_>>();
            let mut refresh = commits_for_objects(&repository.common_dir, &changed_objects)?;
            refresh.extend(commits_for_reachable_parents(
                &repository.common_dir,
                &reachable_commits,
            )?);
            refresh.retain(|commit| reachable.contains(commit));
            refresh.sort();
            refresh.dedup();
            let snapshot = repository.read_incremental_history_at(
                HistoryTarget {
                    tip: expected.tip.clone(),
                    object_format: expected.object_format.clone(),
                    shallow_boundaries: expected.shallow_boundaries.clone(),
                },
                &state.commits,
                &refresh,
                missing_objects,
                report,
            )?;
            report(IndexStage::WritingCache);
            write::append(&super::cache_path(&repository.common_dir), &snapshot)?;
            current_head_commit_count
        }
        Plan::Fresh { .. } => unreachable!("fresh plans return before publishing"),
    };
    drop(exclusive);

    let shared = super::acquire_shared(&repository.common_dir, &mut progress)?;
    let state = match inspect(&repository.common_dir) {
        Inspection::Ready(state) if publication_matches_target(&state, &expected) => state,
        _ => {
            return Err(AppError::operational(
                "error: cache publication did not produce the pinned generation; retry",
            ));
        }
    };
    add_warnings(&mut progress, &expected, &state.missing_objects);
    Ok(PreparedCache {
        progress,
        current_head_commit_count,
        semantic_enabled: state.semantic_enabled,
        _lock: shared,
    })
}

fn evaluate(
    repository: &Repository,
    expected: &Expected,
    inspection: Inspection,
) -> Result<Plan, AppError> {
    let semantic_enabled = match &inspection {
        Inspection::Ready(state) => state.semantic_enabled,
        Inspection::Stale { semantic_enabled } => *semantic_enabled,
        Inspection::Damaged { semantic_enabled } => *semantic_enabled,
        _ => false,
    };
    let Inspection::Ready(state) = inspection else {
        return Ok(Plan::Rebuild {
            damaged: matches!(inspection, Inspection::Damaged { .. }),
            semantic_enabled,
        });
    };

    if state.object_format != expected.object_format {
        return Ok(Plan::Rebuild {
            damaged: false,
            semantic_enabled: state.semantic_enabled,
        });
    }

    let reachable_commits = repository.reachable_commits(&expected.tip)?;
    let current_missing = repository.missing_objects(&state.referenced_objects)?;
    let previous_missing = state
        .missing_objects
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let current_missing_set = current_missing.iter().cloned().collect::<HashSet<_>>();
    let mut changed_objects = current_missing
        .iter()
        .filter(|object| !previous_missing.contains(*object))
        .cloned()
        .collect::<Vec<_>>();
    changed_objects.extend(
        state
            .missing_objects
            .iter()
            .filter(|object| !current_missing_set.contains(*object))
            .cloned(),
    );
    changed_objects.sort();
    changed_objects.dedup();

    if state.tip == expected.tip
        && state.shallow_boundaries == expected.shallow_boundaries
        && changed_objects.is_empty()
    {
        Ok(Plan::Fresh {
            state,
            missing_objects: current_missing,
            current_head_commit_count: reachable_commits.len(),
        })
    } else {
        Ok(Plan::Incremental {
            state,
            missing_objects: current_missing,
            changed_objects,
            reachable_commits,
        })
    }
}

fn add_warnings(progress: &mut Vec<String>, expected: &Expected, missing_objects: &[String]) {
    if let Some(warning) = super::shallow_warning(!expected.shallow_boundaries.is_empty())
        && !progress.contains(&warning)
    {
        progress.push(warning);
    }
    if let Some(warning) = super::missing_warning(!missing_objects.is_empty())
        && !progress.contains(&warning)
    {
        progress.push(warning);
    }
}

fn inspect(common_dir: &Path) -> Inspection {
    let path = super::cache_path(common_dir);
    if !path.is_file() {
        return Inspection::Missing;
    }
    let Ok(connection) = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return Inspection::Damaged {
            semantic_enabled: false,
        };
    };
    let semantic_enabled = matches!(
        metadata(&connection, "semantic_enabled")
            .ok()
            .flatten()
            .as_deref(),
        Some("1")
    );
    if metadata(&connection, "schema_version")
        .ok()
        .flatten()
        .is_some_and(|version| version != SCHEMA_VERSION)
    {
        return Inspection::Stale { semantic_enabled };
    }
    match inspect_connection(&connection) {
        Ok(state) => Inspection::Ready(state),
        Err(()) => Inspection::Damaged { semantic_enabled },
    }
}

fn inspect_path(path: &Path) -> Result<CacheState, AppError> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| cache_error("opening cache for validation", error))?;
    inspect_connection(&connection)
        .map_err(|()| AppError::operational("error: validating cache generation; retry"))
}

fn inspect_connection(connection: &Connection) -> Result<CacheState, ()> {
    let check: String = connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|_| ())?;
    if check != "ok" {
        return Err(());
    }

    for table in [
        "metadata",
        "shallow_boundaries",
        "missing_objects",
        "commits",
        "search_fts",
        "commit_parents",
        "changes",
        "hunks",
        "commit_paths",
        "semantic_vectors",
        "commit_path_counts",
        "hunk_line_blocks",
        "hunk_token_blocks",
        "hunk_payloads",
    ] {
        let exists: Option<String> = connection
            .query_row(
                "SELECT name FROM sqlite_master WHERE name = ?1",
                [table],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| ())?;
        if exists.is_none() {
            return Err(());
        }
    }

    let version = metadata(connection, "schema_version")?.ok_or(())?;
    if version != SCHEMA_VERSION {
        return Err(());
    }
    let object_format = metadata(connection, "object_format")?.ok_or(())?;
    let tip = metadata(connection, "completed_tip")?.ok_or(())?;
    let completed_count = metadata(connection, "completed_commit_count")?
        .ok_or(())?
        .parse::<usize>()
        .map_err(|_| ())?;
    let semantic_enabled = match metadata(connection, "semantic_enabled")?.as_deref() {
        Some("0") => false,
        Some("1") => true,
        _ => return Err(()),
    };

    let shallow_boundaries = string_rows(
        connection,
        "SELECT oid FROM shallow_boundaries ORDER BY oid",
    )?;
    let missing_objects = string_rows(connection, "SELECT oid FROM missing_objects ORDER BY oid")?;
    let commits = string_rows(connection, "SELECT oid FROM commits ORDER BY oid")?;
    let referenced_objects = string_rows(
        connection,
        "SELECT old_blob FROM changes WHERE old_blob IS NOT NULL
         UNION SELECT new_blob FROM changes WHERE new_blob IS NOT NULL
         ORDER BY 1",
    )?;

    let commit_count = count(connection, "SELECT COUNT(*) FROM commits")?;
    let fts_count = count(connection, "SELECT COUNT(*) FROM search_fts")?;
    let fts_document_count = count(connection, "SELECT COUNT(*) FROM search_fts_docsize")?;
    let path_count_rows = count(connection, "SELECT COUNT(*) FROM commit_path_counts")?;
    if commit_count != completed_count as i64
        || path_count_rows != commit_count
        || fts_count != commit_count
        || fts_document_count != commit_count
    {
        return Err(());
    }

    let commit_count = usize::try_from(commit_count).map_err(|_| ())?;
    if commits.len() != commit_count {
        return Err(());
    }
    Ok(CacheState {
        tip,
        object_format,
        shallow_boundaries,
        missing_objects,
        referenced_objects,
        commits,
        commit_count,
        semantic_enabled,
    })
}

fn metadata(connection: &Connection, key: &str) -> Result<Option<String>, ()> {
    connection
        .query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|_| ())
}

fn string_rows(connection: &Connection, query: &str) -> Result<Vec<String>, ()> {
    let mut statement = connection.prepare(query).map_err(|_| ())?;
    statement
        .query_map([], |row| row.get(0))
        .map_err(|_| ())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ())
}

fn count(connection: &Connection, query: &str) -> Result<i64, ()> {
    connection
        .query_row(query, [], |row| row.get(0))
        .map_err(|_| ())
}

fn preserve_damaged(common_dir: &Path) -> Result<(), AppError> {
    let directory = super::cache_directory(common_dir);
    let path = directory.join("cache.sqlite");
    if !path.exists() {
        return Ok(());
    }
    let preserved = directory.join(format!(
        "cache.sqlite.corrupt-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::copy(path, preserved).map_err(|error| cache_error("preserving damaged cache", error))?;
    Ok(())
}

fn recover_previous(common_dir: &Path) -> Result<(), AppError> {
    let directory = super::cache_directory(common_dir);
    let final_path = directory.join("cache.sqlite");
    let previous = directory.join("cache.sqlite.previous");
    if !final_path.exists() && previous.exists() {
        fs::rename(previous, final_path)
            .map_err(|error| cache_error("recovering the previous cache", error))?;
    }
    Ok(())
}

fn publish(common_dir: &Path, snapshot: &Snapshot, semantic_enabled: bool) -> Result<(), AppError> {
    let directory = super::cache_directory(common_dir);
    fs::create_dir_all(&directory)
        .map_err(|error| cache_error("creating shared cache directory", error))?;
    super::ensure_ignored(&directory)?;

    let final_path = directory.join("cache.sqlite");
    let staging = directory.join(format!("cache.sqlite.staging-{}", std::process::id()));
    let _ = fs::remove_file(&staging);
    if let Err(error) = write::build(&staging, snapshot, semantic_enabled) {
        let _ = fs::remove_file(&staging);
        return Err(error);
    }
    if semantic_enabled {
        super::semantic::copy_semantic_vectors(&final_path, &staging);
    }
    if let Err(error) = validate(&staging, snapshot.commits.len()) {
        let _ = fs::remove_file(&staging);
        return Err(error);
    }
    atomic_replace(&directory, &final_path, &staging)
}

fn validate(path: &Path, expected_commits: usize) -> Result<(), AppError> {
    let state = inspect_path(path)?;
    if state.commit_count != expected_commits {
        return Err(AppError::operational(
            "error: validating staging cache failed; retry",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn atomic_replace(directory: &Path, final_path: &Path, staging: &Path) -> Result<(), AppError> {
    if final_path.exists() {
        let previous = directory.join("cache.sqlite.previous");
        let _ = fs::remove_file(&previous);
        fs::rename(final_path, &previous)
            .map_err(|error| cache_error("preserving the previous cache", error))?;
        if let Err(error) = fs::rename(staging, final_path) {
            let _ = fs::rename(&previous, final_path);
            let _ = fs::remove_file(staging);
            return Err(cache_error("publishing the cache", error));
        }
        let _ = fs::remove_file(previous);
    } else {
        fs::rename(staging, final_path)
            .map_err(|error| cache_error("publishing the cache", error))?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn atomic_replace(_directory: &Path, final_path: &Path, staging: &Path) -> Result<(), AppError> {
    fs::rename(staging, final_path).map_err(|error| cache_error("publishing the cache", error))
}

fn commits_for_objects(common_dir: &Path, objects: &[String]) -> Result<Vec<String>, AppError> {
    if objects.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", objects.len())
        .collect::<Vec<_>>()
        .join(", ");
    let query = format!(
        "SELECT DISTINCT c.oid
         FROM changes AS ch
         JOIN commits AS c ON c.commit_id = ch.commit_id
         WHERE ch.old_blob IN ({placeholders}) OR ch.new_blob IN ({placeholders})
         ORDER BY c.oid",
    );
    let values = objects.iter().chain(objects.iter());
    let connection = Connection::open_with_flags(
        super::cache_path(common_dir),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|error| cache_error("opening cache", error))?;
    let mut statement = connection
        .prepare(&query)
        .map_err(|error| cache_error("preparing missing-object lookup", error))?;
    statement
        .query_map(params_from_iter(values), |row| row.get(0))
        .map_err(|error| cache_error("reading missing-object lookup", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| cache_error("reading missing-object lookup", error))
}

fn commits_for_reachable_parents(
    common_dir: &Path,
    reachable_commits: &[String],
) -> Result<Vec<String>, AppError> {
    const CHUNK_SIZE: usize = 500;
    if reachable_commits.is_empty() {
        return Ok(Vec::new());
    }

    let connection = Connection::open_with_flags(
        super::cache_path(common_dir),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|error| cache_error("opening cache", error))?;
    let mut commits = Vec::new();
    for parent_oids in reachable_commits.chunks(CHUNK_SIZE) {
        let placeholders = std::iter::repeat_n("?", parent_oids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let query = format!(
            "SELECT DISTINCT child.oid
             FROM commit_parents AS parent_link
             JOIN commits AS child ON child.commit_id = parent_link.commit_id
             WHERE parent_link.parent_id IS NULL
               AND parent_link.external_oid IN ({placeholders})
             ORDER BY child.oid"
        );
        let mut statement = connection
            .prepare(&query)
            .map_err(|error| cache_error("preparing unresolved-parent lookup", error))?;
        let rows = statement
            .query_map(params_from_iter(parent_oids), |row| row.get(0))
            .map_err(|error| cache_error("reading unresolved-parent lookup", error))?;
        commits.extend(
            rows.collect::<Result<Vec<String>, _>>()
                .map_err(|error| cache_error("reading unresolved-parent lookup", error))?,
        );
    }
    commits.sort();
    commits.dedup();
    Ok(commits)
}

#[cfg(test)]
mod tests {
    use super::{CacheState, Expected, publication_matches_target};

    #[test]
    fn publication_accepts_a_pinned_head_cached_before_another_head() {
        let expected = Expected {
            tip: "pinned-tip".to_owned(),
            object_format: "sha1".to_owned(),
            shallow_boundaries: Vec::new(),
        };
        let state = CacheState {
            tip: "other-tip".to_owned(),
            object_format: "sha1".to_owned(),
            shallow_boundaries: Vec::new(),
            missing_objects: Vec::new(),
            referenced_objects: Vec::new(),
            commits: vec!["pinned-tip".to_owned()],
            commit_count: 1,
            semantic_enabled: false,
        };

        assert!(publication_matches_target(&state, &expected));
    }
}
