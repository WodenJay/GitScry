//! Read-only pinning of an ordinary merge and its unmerged index stages.
use std::{collections::BTreeMap, fs, path::PathBuf};

use super::Repository;
use crate::app::AppError;

pub(crate) struct MergeConflict {
    pub(crate) ours: String,
    pub(crate) theirs: String,
    pub(crate) base: String,
    pub(crate) files: Vec<ConflictFile>,
}

pub(crate) struct ConflictFile {
    pub(crate) path: Vec<u8>,
    pub(crate) unsupported: Option<String>,
}

fn metadata(repository: &Repository, name: &str) -> Result<PathBuf, AppError> {
    Ok(PathBuf::from(
        repository
            .git
            .text(["rev-parse", "--path-format=absolute", "--git-path", name])?
            .trim(),
    ))
}

pub(super) fn pin(repository: &Repository) -> Result<MergeConflict, AppError> {
    for name in [
        "rebase-merge",
        "rebase-apply",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "sequencer",
    ] {
        if metadata(repository, name)?.exists() {
            return Err(AppError::input(
                "conflicts supports only an ordinary merge; rebase, cherry-pick and sequenced operations are unsupported",
            ));
        }
    }
    let merge_head = fs::read_to_string(metadata(repository, "MERGE_HEAD")?).map_err(|_| {
        AppError::input("conflicts requires an in-progress ordinary merge (MERGE_HEAD)")
    })?;
    let endpoints: Vec<_> = merge_head.split_whitespace().collect();
    if endpoints.len() != 1 {
        return Err(AppError::input(
            "conflicts requires exactly two endpoints; multiple merge endpoints are unsupported",
        ));
    }
    let ours = repository.resolve_commit("HEAD")?;
    let theirs = repository.resolve_commit(endpoints[0])?;
    // merge-base exits 1 for unrelated histories, with no output.
    let has_base = repository
        .git
        .success(["merge-base", "--all", &ours, &theirs])?;
    let bases: Vec<String> = if has_base {
        repository
            .git
            .text(["merge-base", "--all", &ours, &theirs])?
            .split_whitespace()
            .map(str::to_owned)
            .collect()
    } else {
        Vec::new()
    };
    if bases.len() != 1 {
        return Err(AppError::input(
            "conflicts requires exactly one merge base; unrelated histories and multiple merge bases are unsupported",
        ));
    }
    let entries = repository
        .git
        .output(["ls-files", "--unmerged", "-z"], &[])?;
    let mut stages = BTreeMap::<Vec<u8>, BTreeMap<u8, (String, String)>>::new();
    for entry in entries
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let tab = entry
            .iter()
            .position(|byte| *byte == b'\t')
            .ok_or_else(|| AppError::operational("error: invalid unmerged index entry"))?;
        let header = String::from_utf8_lossy(&entry[..tab]);
        let fields: Vec<_> = header.split_whitespace().collect();
        if fields.len() != 3 {
            return Err(AppError::operational(
                "error: invalid unmerged index stages",
            ));
        }
        let stage = fields[2]
            .parse::<u8>()
            .map_err(|_| AppError::operational("error: invalid unmerged index stage"))?;
        stages
            .entry(entry[tab + 1..].to_vec())
            .or_default()
            .insert(stage, (fields[0].to_owned(), fields[1].to_owned()));
    }
    let mut files = Vec::new();
    for (path, entries) in stages {
        let mut unsupported = None;
        if entries.len() != 3 || !(1..=3).all(|stage| entries.contains_key(&stage)) {
            unsupported = Some(
                "file-level conflict (addition, deletion or rename); not supported in this slice"
                    .to_owned(),
            );
        } else if entries
            .values()
            .any(|(mode, _)| !matches!(mode.as_str(), "100644" | "100755"))
        {
            unsupported = Some("non-regular file conflict; not supported in this slice".to_owned());
        } else {
            for (_, blob) in entries.values() {
                match repository
                    .git
                    .limited_output(["cat-file", "blob", blob], 1024 * 1024)?
                {
                    Some(bytes) if !bytes.contains(&0) => (),
                    Some(_) => {
                        unsupported =
                            Some("binary conflict; not supported in this slice".to_owned());
                        break;
                    }
                    None => {
                        unsupported =
                            Some("conflict blob exceeds 1 MiB inspection bound".to_owned());
                        break;
                    }
                }
            }
        }
        files.push(ConflictFile { path, unsupported });
    }
    Ok(MergeConflict {
        ours,
        theirs,
        base: bases[0].clone(),
        files,
    })
}
