//! Conservative incarnation tracking over native range history and local source objects.
use std::collections::HashSet;

use super::{
    SymbolChange, SymbolTrace, WhyAnchor, WhyTarget, history, process::Git, symbol, target,
};
use crate::app::AppError;

pub(super) fn trace(git: &Git, target: &WhyTarget) -> SymbolTrace {
    let mut trace = SymbolTrace {
        revisions: Vec::new(),
        modifications: Vec::new(),
        paths: Vec::new(),
        introduction: Err("Git returned no history for the symbol range".to_owned()),
    };
    trace.introduction = walk(git, target, &mut trace).map_err(introduction_error);
    trace
}

fn unknown(reason: &str) -> AppError {
    AppError::operational(reason)
}

fn introduction_error(error: AppError) -> String {
    let message = error.to_string();
    let lowercase = message.to_ascii_lowercase();
    let missing_object = [
        "unable to read ",
        "bad object ",
        "missing object",
        "no such object",
        "could not read ",
        "could not get object info",
        "object file ",
        "not a valid object name",
    ]
    .iter()
    .any(|needle| lowercase.contains(needle));

    if missing_object {
        "required Git objects are missing or unreadable; symbol introduction cannot be verified"
            .to_owned()
    } else {
        message
    }
}

fn walk(git: &Git, target: &WhyTarget, trace: &mut SymbolTrace) -> Result<String, AppError> {
    let WhyAnchor::Symbol { name, number } = &target.anchor else {
        return Err(unknown("target is not a symbol"));
    };
    let mut name = name.clone();
    let mut path = std::str::from_utf8(&target.path)
        .map_err(|_| unknown("symbol path is not UTF-8"))?
        .to_owned();
    let mut revision = target.revision.clone();
    let mut span = symbol::Span {
        start: *number,
        end: target.symbol_end.unwrap_or(*number),
    };
    let shallow = history::read_shallow_boundaries(git)?;
    let mut merge_uncertain = false;
    loop {
        trace.paths.push(path.as_bytes().to_vec());
        let native = history::trace_symbol(git, &revision, &path, span.start, span.end)?
            .into_iter()
            .collect::<HashSet<_>>();
        let commits = git.text([
            "--literal-pathspecs",
            "log",
            "--first-parent",
            "--format=%H",
            &revision,
            "--",
            &path,
        ])?;
        let mut continued = false;
        for oid in commits.lines() {
            let (commit, changes, _) = history::read_trace_fix(git, oid)?;
            let current = target::read_blob_at(git, oid, &path)?
                .ok_or_else(|| unknown("historical symbol source is unavailable"))?;
            let current_span = symbol::locate_unique(&current, &name, &path)?;
            if commit.parents.len() > 1 {
                let first_parent = &commit.parents[0];
                if let Some(source) = target::read_blob_at(git, first_parent, &path)?
                    && let Ok(previous_span) = symbol::locate_unique(&source, &name, &path)
                {
                    if symbol::identity(&current, &current_span)
                        != symbol::identity(&source, &previous_span)
                    {
                        // Keep tracing first-parent edits, but don't claim a verified origin.
                        merge_uncertain = true;
                    }
                    span = previous_span.span;
                    revision = first_parent.clone();
                    continued = true;
                    break;
                }
                return Err(unknown("merge history makes symbol continuity uncertain"));
            }
            if native.contains(oid) {
                trace.revisions.push(oid.to_owned());
            }
            if shallow.iter().any(|boundary| boundary == oid) {
                if native.contains(oid) {
                    trace.modifications.push(SymbolChange {
                        oid: oid.to_owned(),
                        path: path.as_bytes().to_vec(),
                        start: current_span.span.start,
                        end: current_span.span.end,
                    });
                }
                return Err(unknown(
                    "shallow history prevents confirming symbol introduction",
                ));
            }
            let Some(parent) = commit.parents.first() else {
                if merge_uncertain {
                    return Err(unknown("merge history makes symbol continuity uncertain"));
                }
                return Ok(oid.to_owned());
            };
            let previous = target::read_blob_at(git, parent, &path)?;
            if let Some(source) = &previous
                && let Ok(previous_span) = symbol::locate_unique(source, &name, &path)
            {
                if native.contains(oid)
                    && symbol::identity(&current, &current_span)
                        != symbol::identity(source, &previous_span)
                {
                    let changes = changes.as_ref().ok_or_else(|| {
                        unknown("source changes unavailable for symbol continuity")
                    })?;
                    for change in changes {
                        let Some(old_path) = change.old_path.as_deref() else {
                            continue;
                        };
                        let Ok(old_path) = std::str::from_utf8(old_path) else {
                            continue;
                        };
                        let Some(source) = target::read_blob_at(git, parent, old_path)? else {
                            continue;
                        };
                        let after = target::read_blob_at(git, oid, old_path)?;
                        for old_span in symbol::declarations(&source, old_path).0 {
                            let old_name = &old_span.name;
                            if old_path == path && old_name == &name {
                                continue;
                            }
                            if symbol::identity(&source, &old_span)
                                == symbol::identity(&current, &current_span)
                                && after.as_ref().is_none_or(|after| {
                                    symbol::declaration_lines(after, old_name, old_path).is_empty()
                                })
                            {
                                return Err(unknown(
                                    "ambiguous symbol continuity/origin: same-name replacement has a competing move source",
                                ));
                            }
                        }
                    }
                    trace.modifications.push(SymbolChange {
                        oid: oid.to_owned(),
                        path: path.as_bytes().to_vec(),
                        start: current_span.span.start,
                        end: current_span.span.end,
                    });
                }
                continue;
            }
            let changes = changes
                .ok_or_else(|| unknown("source changes unavailable for symbol continuity"))?;
            let identity = symbol::identity(&current, &current_span);
            let mut candidates = Vec::new();
            let mut removed_declaration = false;
            // Inspect parent source, including unchanged files: an exact copy is not a creation
            // or a move, and a surviving source must never be silently followed.
            let paths = git.output(["ls-tree", "-r", "--name-only", "-z", parent], &[])?;
            for source_path in paths
                .split(|byte| *byte == 0)
                .filter(|path| !path.is_empty())
            {
                let Ok(source_path) = std::str::from_utf8(source_path) else {
                    continue;
                };
                let Some(source) = target::read_blob_at(git, parent, source_path)? else {
                    continue;
                };
                let changed = changes
                    .iter()
                    .any(|change| change.old_path.as_deref() == Some(source_path.as_bytes()));
                let after = if changed {
                    target::read_blob_at(git, oid, source_path)?
                } else {
                    None
                };
                for old_span in symbol::declarations(&source, source_path).0 {
                    let old_name = &old_span.name;
                    let survives = !changed
                        || after.as_ref().is_some_and(|after| {
                            !symbol::declaration_lines(after, old_name, source_path).is_empty()
                        });
                    if changed
                        && !survives
                        && (old_name == &name
                            || source_path == path
                            || previous.is_some()
                            || changes.iter().any(|change| {
                                change.status.starts_with('R')
                                    && change.old_path.as_deref() == Some(source_path.as_bytes())
                                    && change.new_path.as_deref() == Some(path.as_bytes())
                            }))
                    {
                        removed_declaration = true;
                    }
                    if symbol::identity(&source, &old_span) == identity {
                        candidates.push((
                            source_path.to_owned(),
                            old_name.clone(),
                            old_span,
                            changed && !survives,
                        ));
                    }
                }
            }
            if let [(old_path, old_name, old_span, true)] = candidates.as_slice() {
                path = old_path.clone();
                name = old_name.clone();
                span = symbol::Span {
                    start: old_span.span.start,
                    end: old_span.span.end,
                };
                revision = parent.clone();
                continued = true;
                break;
            }
            if !candidates.is_empty() {
                return Err(unknown(
                    "ambiguous symbol continuity/origin: multiple candidates or surviving copy source",
                ));
            }
            if removed_declaration {
                return Err(unknown(
                    "symbol continuity/origin is unknown: materially rewritten relocation",
                ));
            }
            return Ok(oid.to_owned());
        }
        if !continued {
            return Err(unknown(
                "Git returned no verified introduction for the symbol range",
            ));
        }
    }
}
