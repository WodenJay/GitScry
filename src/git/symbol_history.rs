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
        introduction: Err("Git returned no history for the symbol range".to_owned()),
    };
    trace.introduction = walk(git, target, &mut trace).map_err(|error| error.to_string());
    trace
}

fn unknown(reason: &str) -> AppError {
    AppError::operational(reason)
}

fn walk(git: &Git, target: &WhyTarget, trace: &mut SymbolTrace) -> Result<String, AppError> {
    let WhyAnchor::Symbol { name, number } = &target.anchor else {
        return Err(unknown("target is not a symbol"));
    };
    let mut name = name.clone();
    let path =
        std::str::from_utf8(&target.path).map_err(|_| unknown("symbol path is not UTF-8"))?;
    let mut revision = target.revision.clone();
    let mut span = symbol::Span {
        start: *number,
        end: target.symbol_end.unwrap_or(*number),
    };
    let shallow = history::read_shallow_boundaries(git)?;
    loop {
        let native = history::trace_symbol(git, &revision, path, span.start, span.end)?
            .into_iter()
            .collect::<HashSet<_>>();
        let commits = git.text([
            "log",
            "--first-parent",
            "--format=%H",
            &revision,
            "--",
            path,
        ])?;
        let mut continued = false;
        for oid in commits.lines() {
            let (commit, _, _) = history::read_trace_fix(git, oid)?;
            if commit.parents.len() > 1 {
                return Err(unknown("merge history makes symbol continuity uncertain"));
            }
            let current = target::read_blob_at(git, oid, path)?
                .ok_or_else(|| unknown("historical symbol source is unavailable"))?;
            let current_span = symbol::locate_unique(&current, &name, path)?;
            if native.contains(oid) {
                trace.revisions.push(oid.to_owned());
            }
            if shallow.iter().any(|boundary| boundary == oid) {
                if native.contains(oid) {
                    trace.modifications.push(SymbolChange {
                        oid: oid.to_owned(),
                        path: path.as_bytes().to_vec(),
                        start: current_span.start,
                        end: current_span.end,
                    });
                }
                return Err(unknown(
                    "shallow history prevents confirming symbol introduction",
                ));
            }
            let Some(parent) = commit.parents.first() else {
                return Ok(oid.to_owned());
            };
            let previous = target::read_blob_at(git, parent, path)?;
            if let Some(source) = &previous {
                if let Ok(previous_span) = symbol::locate_unique(source, &name, path) {
                    if native.contains(oid)
                        && symbol::identity(&current, &name, &current_span)
                            != symbol::identity(source, &name, &previous_span)
                    {
                        trace.modifications.push(SymbolChange {
                            oid: oid.to_owned(),
                            path: path.as_bytes().to_vec(),
                            start: current_span.start,
                            end: current_span.end,
                        });
                    }
                    continue;
                }
                let identity = symbol::identity(&current, &name, &current_span);
                let candidates = symbol::declarations(source, path)
                    .into_iter()
                    .filter(|(old_name, old_span)| {
                        symbol::identity(source, old_name, old_span) == identity
                    })
                    .collect::<Vec<_>>();
                if let [(old_name, old_span)] = candidates.as_slice() {
                    name = old_name.clone();
                    span = symbol::Span {
                        start: old_span.start,
                        end: old_span.end,
                    };
                    revision = parent.clone();
                    continued = true;
                    break;
                }
                if !candidates.is_empty() {
                    return Err(unknown(
                        "ambiguous symbol continuity: multiple source declarations",
                    ));
                }
            }
            if target::verify_symbol_introduction(git, oid, path, &name)? {
                return Ok(oid.to_owned());
            }
            return Err(unknown(
                "symbol continuity/origin is unknown: relocation or rewritten declaration",
            ));
        }
        if !continued {
            return Err(unknown(
                "Git returned no verified introduction for the symbol range",
            ));
        }
    }
}
