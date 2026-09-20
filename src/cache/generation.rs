use std::collections::HashSet;

use crate::{
    app::AppError,
    git::{HistoryTarget, Repository},
};

use super::{CacheState, Inspection, SharedLock};

pub(crate) struct PreparedCache {
    pub(crate) progress: Vec<String>,
    pub(crate) commit_count: usize,
    _lock: SharedLock,
}

struct Expected {
    default_ref: String,
    tip: String,
    object_format: String,
    shallow_boundaries: Vec<String>,
}

enum Plan {
    Fresh {
        state: CacheState,
        missing_objects: Vec<String>,
    },
    Incremental {
        state: CacheState,
        missing_objects: Vec<String>,
        newly_available: Vec<String>,
    },
    Rebuild {
        damaged: bool,
    },
}

pub(crate) fn prepare(repository: &Repository) -> Result<PreparedCache, AppError> {
    let (default_ref, tip) = repository.default_target()?;
    prepare_at(repository, default_ref, tip)
}

pub(crate) fn prepare_at(
    repository: &Repository,
    default_ref: String,
    tip: String,
) -> Result<PreparedCache, AppError> {
    let expected = Expected {
        default_ref,
        tip,
        object_format: repository.object_format()?,
        shallow_boundaries: repository.shallow_boundaries()?,
    };
    let mut progress = Vec::new();
    let shared = super::acquire_shared(&repository.root, &mut progress)?;
    let plan = evaluate(repository, &expected, super::inspect(&repository.root))?;
    if let Plan::Fresh {
        state,
        missing_objects,
    } = plan
    {
        add_warnings(&mut progress, &expected, &missing_objects);
        return Ok(PreparedCache {
            progress,
            commit_count: state.commit_count,
            _lock: shared,
        });
    }
    drop(shared);

    let exclusive = super::acquire_exclusive(&repository.root, &mut progress)?;
    super::recover_previous(&repository.root)?;
    let plan = evaluate(repository, &expected, super::inspect(&repository.root))?;
    if let Plan::Fresh {
        state,
        missing_objects,
    } = plan
    {
        drop(exclusive);
        let shared = super::acquire_shared(&repository.root, &mut progress)?;
        add_warnings(&mut progress, &expected, &missing_objects);
        return Ok(PreparedCache {
            progress,
            commit_count: state.commit_count,
            _lock: shared,
        });
    }

    match plan {
        Plan::Rebuild { damaged } => {
            if damaged {
                super::preserve_damaged(&repository.root)?;
            }
            progress.push("Indexing local history...".to_owned());
            let snapshot = repository.read_default_history_at(HistoryTarget {
                default_ref: expected.default_ref.clone(),
                tip: expected.tip.clone(),
                object_format: expected.object_format.clone(),
                shallow_boundaries: expected.shallow_boundaries.clone(),
            })?;
            let count = snapshot.commits.len();
            super::publish(&repository.root, &snapshot)?;
            count
        }
        Plan::Incremental {
            state,
            missing_objects,
            newly_available,
        } => {
            progress.push("Indexing local history...".to_owned());
            let mut refresh = Vec::new();
            if state.shallow_boundaries != expected.shallow_boundaries {
                refresh.extend(super::boundary_refreshes(&repository.root)?);
            }
            refresh.extend(super::commits_for_objects(
                &repository.root,
                &newly_available,
            )?);
            refresh.sort();
            refresh.dedup();
            let snapshot = repository.read_incremental_history_at(
                HistoryTarget {
                    default_ref: expected.default_ref.clone(),
                    tip: expected.tip.clone(),
                    object_format: expected.object_format.clone(),
                    shallow_boundaries: expected.shallow_boundaries.clone(),
                },
                &state.commits,
                &refresh,
                missing_objects,
            )?;
            super::append(&repository.root, &snapshot)?
        }
        Plan::Fresh { .. } => unreachable!("fresh plans return before publishing"),
    };
    drop(exclusive);

    let shared = super::acquire_shared(&repository.root, &mut progress)?;
    let state = match super::inspect(&repository.root) {
        Inspection::Ready(state)
            if state.default_ref == expected.default_ref
                && state.tip == expected.tip
                && state.object_format == expected.object_format
                && state.shallow_boundaries == expected.shallow_boundaries =>
        {
            state
        }
        _ => {
            return Err(AppError::operational(
                "error: cache publication did not produce the pinned generation; retry",
            ));
        }
    };
    add_warnings(&mut progress, &expected, &state.missing_objects);
    Ok(PreparedCache {
        progress,
        commit_count: state.commit_count,
        _lock: shared,
    })
}

fn evaluate(
    repository: &Repository,
    expected: &Expected,
    inspection: Inspection,
) -> Result<Plan, AppError> {
    let Inspection::Ready(state) = inspection else {
        return Ok(Plan::Rebuild {
            damaged: matches!(inspection, Inspection::Damaged),
        });
    };

    if !repository.missing_objects(&state.commits)?.is_empty() {
        return Ok(Plan::Rebuild { damaged: false });
    }
    let current_missing = repository.missing_objects(&state.referenced_objects)?;
    let previous_missing = state.missing_objects.iter().collect::<HashSet<_>>();
    let current_missing_set = current_missing.iter().collect::<HashSet<_>>();
    let disappeared = current_missing
        .iter()
        .any(|object| !previous_missing.contains(object));
    let newly_available = state
        .missing_objects
        .iter()
        .filter(|object| !current_missing_set.contains(object))
        .cloned()
        .collect::<Vec<_>>();

    if disappeared
        || state.default_ref != expected.default_ref
        || state.object_format != expected.object_format
    {
        return Ok(Plan::Rebuild { damaged: false });
    }

    let shallow_is_usable = if state.shallow_boundaries == expected.shallow_boundaries {
        true
    } else if state.shallow_boundaries.is_empty() {
        false
    } else {
        let missing_boundary = repository.missing_objects(&state.shallow_boundaries)?;
        let reachable = repository.reachable_commits(&expected.tip)?;
        let reachable = reachable.into_iter().collect::<HashSet<_>>();
        missing_boundary.is_empty()
            && state
                .commits
                .iter()
                .all(|commit| reachable.contains(commit))
            && state.shallow_boundaries.iter().all(|boundary| {
                repository
                    .is_ancestor(boundary, &expected.tip)
                    .unwrap_or(false)
            })
    };
    if !shallow_is_usable {
        return Ok(Plan::Rebuild { damaged: false });
    }

    let tip_is_forward = if state.tip == expected.tip {
        true
    } else if repository
        .missing_objects(std::slice::from_ref(&state.tip))?
        .is_empty()
    {
        repository.is_ancestor(&state.tip, &expected.tip)?
    } else {
        false
    };
    if !tip_is_forward {
        return Ok(Plan::Rebuild { damaged: false });
    }

    if state.tip == expected.tip
        && state.shallow_boundaries == expected.shallow_boundaries
        && newly_available.is_empty()
    {
        Ok(Plan::Fresh {
            state,
            missing_objects: current_missing,
        })
    } else {
        Ok(Plan::Incremental {
            state,
            missing_objects: current_missing,
            newly_available,
        })
    }
}

fn add_warnings(progress: &mut Vec<String>, expected: &Expected, missing_objects: &[String]) {
    if let Some(warning) = super::shallow_warning(&expected.shallow_boundaries)
        && !progress.contains(&warning)
    {
        progress.push(warning);
    }
    if let Some(warning) = super::missing_warning(missing_objects)
        && !progress.contains(&warning)
    {
        progress.push(warning);
    }
}
