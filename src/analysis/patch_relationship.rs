//! Strict complete-patch identity. Display excerpts never enter this module.
use crate::{
    app::AppError,
    cache::PatchFingerprints,
    git::{Change, CurrentPatch, Hunk, Repository},
};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub(crate) const NORMALIZATION_VERSION: i64 = 3;

#[derive(Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FilePatch {
    old_path: Option<Vec<u8>>,
    new_path: Option<Vec<u8>>,
    operation: char,
    old_mode: String,
    new_mode: String,
    edits: Vec<Vec<u8>>,
}

impl FilePatch {
    fn inverse(&self) -> Result<Self, String> {
        let operation = match self.operation {
            'A' => 'D',
            'D' => 'A',
            'M' | 'R' => self.operation,
            _ => return Err("unsupported file operation".into()),
        };
        Ok(Self {
            old_path: self.new_path.clone(),
            new_path: self.old_path.clone(),
            operation,
            old_mode: self.new_mode.clone(),
            new_mode: self.old_mode.clone(),
            edits: self
                .edits
                .iter()
                .map(|edit| inverse_edit(edit))
                .collect::<Result<_, _>>()?,
        })
    }
}

pub(crate) struct CompletePatch {
    pub(crate) normalized: Vec<FilePatch>,
    pub(crate) comparison_basis: &'static str,
    pub(crate) parent: Option<String>,
    pub(crate) paths: Vec<Vec<u8>>,
    pub(crate) hunks: Vec<Hunk>,
    pub(crate) objects: Vec<String>,
}

impl CompletePatch {
    pub(crate) fn is_empty(&self) -> bool {
        self.normalized.is_empty()
    }

    pub(crate) fn fingerprint(&self) -> Vec<u8> {
        fingerprint(&self.normalized)
    }

    pub(crate) fn inverse_fingerprint(&self) -> Vec<u8> {
        fingerprint(
            &self
                .inverse_normalized()
                .expect("complete patch can be inverted"),
        )
    }

    pub(crate) fn equivalent(&self, other: &Self) -> bool {
        !self.is_empty() && self.normalized == other.normalized
    }

    pub(crate) fn inverse_equivalent(&self, other: &Self) -> bool {
        !self.is_empty()
            && self
                .inverse_normalized()
                .is_ok_and(|inverse| inverse == other.normalized)
    }

    fn inverse_normalized(&self) -> Result<Vec<FilePatch>, String> {
        let mut inverse = self
            .normalized
            .iter()
            .map(FilePatch::inverse)
            .collect::<Result<Vec<_>, _>>()?;
        inverse.sort();
        Ok(inverse)
    }
}

fn fingerprint(files: &[FilePatch]) -> Vec<u8> {
    Sha256::digest(serde_json::to_vec(files).expect("byte patch serialization")).to_vec()
}

fn inverse_edit(edit: &[u8]) -> Result<Vec<u8>, String> {
    let mut blocks = Vec::<(Vec<Vec<u8>>, Vec<Vec<u8>>)>::new();
    let mut side = None;
    for line in edit.split_inclusive(|byte| *byte == b'\n') {
        match line.first() {
            Some(b'-') => {
                if side == Some(true) || blocks.is_empty() {
                    blocks.push((Vec::new(), Vec::new()));
                }
                blocks
                    .last_mut()
                    .expect("created edit block")
                    .0
                    .push(line.to_vec());
                side = Some(false);
            }
            Some(b'+') => {
                if blocks.is_empty() {
                    blocks.push((Vec::new(), Vec::new()));
                }
                blocks
                    .last_mut()
                    .expect("created edit block")
                    .1
                    .push(line.to_vec());
                side = Some(true);
            }
            Some(b'\\') => {
                if line != b"\\ No newline at end of file\n" {
                    return Err("unsupported patch line marker".into());
                }
                let block = blocks.last_mut().ok_or("orphaned no-newline marker")?;
                let changed_line = match side {
                    Some(false) => block.0.last_mut(),
                    Some(true) => block.1.last_mut(),
                    None => None,
                }
                .ok_or("orphaned no-newline marker")?;
                changed_line.extend_from_slice(line);
            }
            _ => return Err("unsupported complete edit line".into()),
        }
    }
    let mut inverse = Vec::new();
    for (removed, added) in blocks {
        for line in added.into_iter().chain(removed) {
            let mut reversed = Vec::with_capacity(line.len());
            reversed.push(match line.first() {
                Some(b'-') => b'+',
                Some(b'+') => b'-',
                _ => return Err("unsupported complete edit line".into()),
            });
            reversed.extend_from_slice(&line[1..]);
            inverse.extend_from_slice(&reversed);
        }
    }
    Ok(inverse)
}

pub(crate) fn inspect(repository: &Repository, oid: &str) -> Result<CompletePatch, String> {
    let (commit, changes, hunks) = repository
        .complete_patch(oid)
        .map_err(|error| error.to_string())?;
    let changes = changes.ok_or("complete file operations unavailable")?;
    let hunks = hunks.ok_or("complete patch unavailable")?;
    let (files, paths) = normalize_changes(&changes, &hunks)?;
    let parent = commit.parents.first().cloned();
    let mut objects = vec![oid.to_owned()];
    objects.extend(parent.iter().cloned());
    objects.extend(
        repository
            .patch_tree_objects(oid, parent.as_deref())
            .map_err(|error| error.to_string())?,
    );
    for change in &changes {
        objects.extend(
            change
                .old_blob
                .iter()
                .chain(change.new_blob.iter())
                .cloned(),
        );
    }
    objects.sort();
    objects.dedup();
    if !repository
        .missing_objects(&objects)
        .map_err(|error| error.to_string())?
        .is_empty()
    {
        return Err("complete patch source objects are unavailable".into());
    }
    Ok(CompletePatch {
        normalized: files,
        objects,
        comparison_basis: if parent.is_some() {
            "first_parent"
        } else {
            "root_introduction"
        },
        parent,
        paths,
        hunks,
    })
}

pub(crate) fn inspect_current(current: CurrentPatch) -> Result<CompletePatch, String> {
    let (normalized, paths) = normalize_changes(&current.changes, &current.hunks)?;
    let comparison_basis = match (current.staged, current.head.is_some()) {
        (true, true) => "head_to_index",
        (false, true) => "head_to_worktree",
        (true, false) => "empty_tree_to_index",
        (false, false) => "empty_tree_to_worktree",
    };
    Ok(CompletePatch {
        normalized,
        comparison_basis,
        parent: current.head,
        paths,
        hunks: current.hunks,
        objects: Vec::new(),
    })
}

fn normalize_changes(
    changes: &[Change],
    hunks: &[Hunk],
) -> Result<(Vec<FilePatch>, Vec<Vec<u8>>), String> {
    let mut files = Vec::new();
    let mut paths = Vec::new();
    for change in changes {
        let file = normalize_file(change, hunks)?;
        // Only patches whose file operations and content can be inverted are complete.
        let _ = file.inverse()?;
        paths.extend(
            change
                .old_path
                .iter()
                .chain(change.new_path.iter())
                .cloned(),
        );
        files.push(file);
    }
    files.sort();
    paths.sort();
    paths.dedup();
    if hunks.iter().any(|hunk| {
        !changes
            .iter()
            .any(|change| change.ordinal == hunk.change_ordinal)
    }) {
        return Err("patch contains an unaccounted file operation".into());
    }
    Ok((files, paths))
}

pub(crate) fn inspect_and_cache(
    repository: &Repository,
    fingerprints: &PatchFingerprints,
    oid: &str,
) -> Result<Result<CompletePatch, String>, AppError> {
    match inspect(repository, oid) {
        Ok(patch) => {
            let fingerprint = patch.fingerprint();
            let inverse_fingerprint = patch.inverse_fingerprint();
            fingerprints.put(
                oid,
                if patch.is_empty() {
                    "empty"
                } else {
                    "complete"
                },
                Some(&fingerprint),
                Some(&inverse_fingerprint),
                &patch.objects,
            )?;
            Ok(Ok(patch))
        }
        Err(reason) => {
            fingerprints.put(oid, "indeterminate", None, None, &[])?;
            Ok(Err(reason))
        }
    }
}

fn normalize_file(change: &Change, hunks: &[Hunk]) -> Result<FilePatch, String> {
    if [&change.old_mode, &change.new_mode]
        .into_iter()
        .any(|mode| !matches!(mode.as_str(), "000000" | "100644" | "100755"))
    {
        return Err("non-regular file material is unsupported".into());
    }
    let mut edits = Vec::new();
    for hunk in hunks
        .iter()
        .filter(|hunk| hunk.change_ordinal == change.ordinal)
    {
        let mut lines = hunk.text.split_inclusive(|byte| *byte == b'\n');
        if !lines.next().is_some_and(|line| line.starts_with(b"@@ ")) {
            return Err("invalid complete hunk".into());
        }
        let mut removed = 0;
        let mut added = 0;
        let mut edit = Vec::new();
        let mut previous_change = false;
        for line in lines {
            match line.first() {
                Some(b'-') => {
                    removed += 1;
                    previous_change = true;
                }
                Some(b'+') => {
                    added += 1;
                    previous_change = true;
                }
                Some(b'\\') if previous_change && line == b"\\ No newline at end of file\n" => {
                    previous_change = false;
                }
                _ => return Err("unsupported or incomplete zero-context hunk".into()),
            }
            edit.extend_from_slice(line);
        }
        if removed != hunk.old_lines || added != hunk.new_lines {
            return Err("truncated complete hunk".into());
        }
        edits.push(edit);
    }
    // Content changes without readable hunks must not disappear (binary, missing
    // blobs or attributes suppressing text). Empty blob additions/deletions are safe.
    let empty_blob = |oid: &Option<String>| {
        oid.as_deref().is_none_or(|oid| {
            matches!(
                oid,
                "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"
                    | "473a0f4c3be8a93681a267e3b1e9a7dcda1185436fe141f7749120a303721813"
            )
        })
    };
    if edits.is_empty()
        && change.old_blob != change.new_blob
        && !(empty_blob(&change.old_blob) && empty_blob(&change.new_blob))
    {
        return Err("binary or unavailable changed content".into());
    }
    Ok(FilePatch {
        old_path: change.old_path.clone(),
        new_path: change.new_path.clone(),
        operation: *change
            .status
            .as_bytes()
            .first()
            .ok_or("missing operation")? as char,
        old_mode: change.old_mode.clone(),
        new_mode: change.new_mode.clone(),
        edits,
    })
}
