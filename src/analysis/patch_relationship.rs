//! Strict complete-patch identity. Display excerpts never enter this module.
use crate::git::{Change, Hunk, Repository};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub(crate) const NORMALIZATION_VERSION: i64 = 1;

#[derive(Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FilePatch {
    old_path: Option<Vec<u8>>,
    new_path: Option<Vec<u8>>,
    operation: char,
    old_mode: String,
    new_mode: String,
    edits: Vec<Vec<u8>>,
}

pub(crate) struct CompletePatch {
    pub(crate) normalized: Vec<FilePatch>,
    pub(crate) comparison_basis: &'static str,
    pub(crate) parent: Option<String>,
    pub(crate) paths: Vec<Vec<u8>>,
    pub(crate) hunks: Vec<Hunk>,
}

impl CompletePatch {
    pub(crate) fn is_empty(&self) -> bool {
        self.normalized.is_empty()
    }
    pub(crate) fn fingerprint(&self) -> Vec<u8> {
        Sha256::digest(serde_json::to_vec(&self.normalized).expect("byte patch serialization"))
            .to_vec()
    }
    pub(crate) fn equivalent(&self, other: &Self) -> bool {
        !self.is_empty() && self.normalized == other.normalized
    }
}

pub(crate) fn inspect(repository: &Repository, oid: &str) -> Result<CompletePatch, String> {
    let (commit, changes, hunks) = repository
        .complete_patch(oid)
        .map_err(|error| error.to_string())?;
    let changes = changes.ok_or("complete file operations unavailable")?;
    let hunks = hunks.ok_or("complete patch unavailable")?;
    let mut files = Vec::new();
    let mut paths = Vec::new();
    for change in &changes {
        let file = normalize_file(change, &hunks)?;
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
    let parent = commit.parents.first().cloned();
    Ok(CompletePatch {
        normalized: files,
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
