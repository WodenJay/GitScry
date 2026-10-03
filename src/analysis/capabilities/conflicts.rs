//! Direct cached file-change evidence, separately oriented to pinned merge sides.
use crate::analysis::{
    query::{Outcome, QueryReport},
    retrieval,
};
use crate::{app::AppError, cache, git};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap, HashSet};

const FILE_LIMIT: usize = 100;
const REGION_LIMIT: usize = 20;
const MESSAGE_LIMIT: usize = 1000;
const ASSOCIATED_CHANGE_SCAN_LIMIT: usize = 64;
const ASSOCIATED_CHANGE_REPORT_LIMIT: usize = 256;
const ASSOCIATED_MATERIAL_LIMIT: usize = 100;
const ASSOCIATED_MATERIALS_PER_LEAD: usize = 8;
const ASSOCIATED_HUNK_SCAN_LIMIT: usize = 8;
const ASSOCIATED_HUNK_BYTE_LIMIT: usize = 16 * 1024;
const ASSOCIATED_HUNK_LIMIT: usize = 3;
const ASSOCIATED_LINE_LIMIT: usize = 4;
const ASSOCIATED_LINE_CHAR_LIMIT: usize = 240;

#[derive(Serialize)]
pub(crate) struct Report {
    pub(crate) schema_version: u8,
    pub(crate) ours: String,
    pub(crate) theirs: String,
    pub(crate) merge_base: String,
    pub(crate) total_unmerged_paths: usize,
    pub(crate) selected_paths: usize,
    pub(crate) files: Vec<File>,
    pub(crate) files_truncated: bool,
    pub(crate) associated_materials: Vec<AssociatedMaterial>,
    pub(crate) associated_materials_truncated: bool,
    pub(crate) coverage_complete: bool,
    pub(crate) limits: Limits,
    pub(crate) limitations: Vec<String>,
    pub(crate) warnings: Vec<String>,
}
#[derive(Serialize)]
pub(crate) struct Limits {
    files: usize,
    leads_per_file_side: usize,
    regions_per_lead: usize,
    associated_materials_per_report: usize,
    associated_materials_per_lead: usize,
    associated_changes_per_report: usize,
    associated_changes_per_lead: usize,
    associated_hunks_per_material: usize,
    associated_lines_per_hunk: usize,
    associated_line_characters: usize,
    message_characters: usize,
}
#[derive(Serialize)]
pub(crate) struct File {
    pub(crate) path: String,
    pub(crate) path_bytes: Vec<u8>,
    pub(crate) unsupported: Option<String>,
    pub(crate) sides: Vec<Side>,
}
#[derive(Serialize)]
pub(crate) struct Side {
    pub(crate) name: &'static str,
    pub(crate) endpoint: String,
    pub(crate) leads: Vec<Lead>,
    pub(crate) total_leads: usize,
    pub(crate) truncated: bool,
}
#[derive(Serialize)]
pub(crate) struct Lead {
    pub(crate) commit: String,
    pub(crate) path: String,
    pub(crate) association: &'static str,
    pub(crate) subject: String,
    pub(crate) recorded_reason: Option<String>,
    pub(crate) reason_source: &'static str,
    pub(crate) message_truncated: bool,
    pub(crate) message_lossy: bool,
    pub(crate) regions: Vec<Region>,
    pub(crate) regions_truncated: bool,
    pub(crate) associated_material_ids: Vec<usize>,
    pub(crate) associated_materials_truncated: bool,
}
#[derive(Serialize)]
pub(crate) struct Region {
    pub(crate) change_ordinal: i64,
    pub(crate) hunk_ordinal: i64,
    pub(crate) old_start: i64,
    pub(crate) old_lines: i64,
    pub(crate) new_start: i64,
    pub(crate) new_lines: i64,
}

#[derive(Serialize)]
pub(crate) struct AssociatedMaterial {
    pub(crate) id: usize,
    pub(crate) commit: String,
    pub(crate) path: String,
    pub(crate) path_bytes: Vec<u8>,
    pub(crate) kind: &'static str,
    pub(crate) association: &'static str,
    pub(crate) shared_identities: Vec<String>,
    pub(crate) hunks: Vec<AssociatedHunk>,
    pub(crate) hunks_truncated: bool,
    pub(crate) associated_with: Vec<AssociatedLead>,
    pub(crate) associations_truncated: bool,
}

#[derive(Serialize)]
pub(crate) struct AssociatedLead {
    pub(crate) conflict_path: String,
    pub(crate) side: &'static str,
    pub(crate) lead_commit: String,
    pub(crate) shared_identities: Vec<String>,
}

#[derive(Serialize)]
pub(crate) struct AssociatedHunk {
    pub(crate) change_ordinal: i64,
    pub(crate) hunk_ordinal: i64,
    pub(crate) old_start: i64,
    pub(crate) old_lines: i64,
    pub(crate) new_start: i64,
    pub(crate) new_lines: i64,
    pub(crate) shared_identities: Vec<String>,
    pub(crate) lines: Vec<String>,
    pub(crate) excerpt_truncated: bool,
}

fn bounded(text: &str) -> (String, bool) {
    (
        text.chars().take(MESSAGE_LIMIT).collect(),
        text.chars().count() > MESSAGE_LIMIT,
    )
}

fn associated_kind(path: &[u8]) -> Option<&'static str> {
    if super::relations::is_test_path(path) {
        return Some("test");
    }
    let path = retrieval::normalize_path(path);
    let name = path.rsplit('/').next().unwrap_or_default();
    if matches!(
        name,
        ".env" | ".editorconfig" | "makefile" | "dockerfile" | "cmakelists.txt"
    ) || name.starts_with(".env.")
    {
        return Some("configuration");
    }
    let extension = name.rsplit_once('.').map(|(_, extension)| extension)?;
    if matches!(
        extension,
        "rs" | "c"
            | "h"
            | "cc"
            | "hh"
            | "cpp"
            | "hpp"
            | "cxx"
            | "hxx"
            | "go"
            | "py"
            | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "ts"
            | "tsx"
            | "java"
            | "kt"
            | "kts"
            | "cs"
            | "rb"
            | "php"
            | "swift"
            | "m"
            | "mm"
            | "scala"
            | "ex"
            | "exs"
            | "erl"
            | "hs"
            | "lua"
            | "sh"
            | "sql"
            | "vue"
            | "svelte"
            | "dart"
            | "zig"
            | "nim"
    ) {
        Some("changed_code")
    } else if matches!(
        extension,
        "toml" | "yaml" | "yml" | "json" | "jsonc" | "xml" | "ini" | "cfg" | "conf" | "properties"
    ) {
        Some("configuration")
    } else {
        None
    }
}

fn changed_line(line: &[u8]) -> Option<&[u8]> {
    if line.starts_with(b"+++") || line.starts_with(b"---") {
        return None;
    }
    match line.first() {
        Some(b'+') | Some(b'-') => Some(&line[1..]),
        _ => None,
    }
}

fn changed_line_signals(text: &[u8]) -> BTreeSet<String> {
    text.split(|byte| *byte == b'\n')
        .filter_map(changed_line)
        .flat_map(retrieval::distinctive_signals)
        .collect()
}

fn matching_material_hunks(
    hunks: Vec<crate::cache::PatchHistoryHunk>,
    conflict_signals: &BTreeSet<String>,
    truncated: &mut bool,
) -> (Vec<AssociatedHunk>, BTreeSet<String>) {
    let mut material_hunks = Vec::new();
    let mut shared_identities = BTreeSet::new();
    for hunk in hunks {
        let Some(text) = hunk.text else {
            *truncated = true;
            continue;
        };
        let mut lines = Vec::new();
        let mut hunk_identities = BTreeSet::new();
        let mut excerpt_truncated = false;
        for line in text.split(|byte| *byte == b'\n') {
            let Some(changed) = changed_line(line) else {
                continue;
            };
            let matching = retrieval::distinctive_signals(changed)
                .intersection(conflict_signals)
                .cloned()
                .collect::<BTreeSet<_>>();
            if matching.is_empty() {
                continue;
            }
            hunk_identities.extend(matching);
            if lines.len() == ASSOCIATED_LINE_LIMIT {
                excerpt_truncated = true;
                continue;
            }
            let decoded = String::from_utf8_lossy(line);
            let too_long = decoded.chars().count() > ASSOCIATED_LINE_CHAR_LIMIT;
            let excerpt = if too_long {
                let mut excerpt = decoded
                    .chars()
                    .take(ASSOCIATED_LINE_CHAR_LIMIT - 1)
                    .collect::<String>();
                excerpt.push('…');
                excerpt_truncated = true;
                excerpt
            } else {
                decoded.into_owned()
            };
            lines.push(excerpt);
        }
        if hunk_identities.is_empty() {
            continue;
        }
        shared_identities.extend(hunk_identities.iter().cloned());
        if material_hunks.len() == ASSOCIATED_HUNK_LIMIT {
            *truncated = true;
            continue;
        }
        material_hunks.push(AssociatedHunk {
            change_ordinal: hunk.change_ordinal,
            hunk_ordinal: hunk.hunk_ordinal,
            old_start: hunk.old_start,
            old_lines: hunk.old_lines,
            new_start: hunk.new_start,
            new_lines: hunk.new_lines,
            shared_identities: hunk_identities.into_iter().collect(),
            lines,
            excerpt_truncated,
        });
    }
    (material_hunks, shared_identities)
}

fn merge_material_hunks(
    existing: &mut Vec<AssociatedHunk>,
    incoming: Vec<AssociatedHunk>,
    hunks_truncated: &mut bool,
) {
    for hunk in incoming {
        if let Some(previous) = existing.iter_mut().find(|previous| {
            previous.change_ordinal == hunk.change_ordinal
                && previous.hunk_ordinal == hunk.hunk_ordinal
        }) {
            previous.excerpt_truncated |= hunk.excerpt_truncated;
            merge_sorted_unique(&mut previous.shared_identities, hunk.shared_identities);
            for line in hunk.lines {
                if previous.lines.contains(&line) {
                    continue;
                }
                if previous.lines.len() == ASSOCIATED_LINE_LIMIT {
                    previous.excerpt_truncated = true;
                    *hunks_truncated = true;
                    break;
                }
                previous.lines.push(line);
            }
        } else if existing.len() == ASSOCIATED_HUNK_LIMIT {
            *hunks_truncated = true;
        } else {
            existing.push(hunk);
        }
    }
}

fn merge_sorted_unique(target: &mut Vec<String>, incoming: impl IntoIterator<Item = String>) {
    for identity in incoming {
        if !target.contains(&identity) {
            target.push(identity);
        }
    }
    target.sort();
}

struct AssociatedMaterialCollector<'a> {
    session: &'a cache::QuerySession,
    conflict_paths: &'a HashSet<Vec<u8>>,
    materials: Vec<AssociatedMaterial>,
    material_indices: HashMap<(String, i64), usize>,
    truncated: bool,
    remaining_changes: usize,
}

impl<'a> AssociatedMaterialCollector<'a> {
    fn new(session: &'a cache::QuerySession, conflict_paths: &'a HashSet<Vec<u8>>) -> Self {
        Self {
            session,
            conflict_paths,
            materials: Vec::new(),
            material_indices: HashMap::new(),
            truncated: false,
            remaining_changes: ASSOCIATED_CHANGE_REPORT_LIMIT,
        }
    }

    fn associate(
        &mut self,
        conflict_path: &[u8],
        side: &'static str,
        commit: &crate::cache::HistoryCommit,
        conflict_hunks: &[crate::cache::HistoryHunk],
    ) -> Result<(Vec<usize>, bool), AppError> {
        let conflict_signals = conflict_hunks
            .iter()
            .flat_map(|hunk| changed_line_signals(&hunk.text))
            .collect::<BTreeSet<_>>();
        if conflict_signals.is_empty() {
            return Ok((Vec::new(), false));
        }

        let mut material_ids = Vec::new();
        let mut truncated = false;
        let mut scanned = 0;
        for change in &commit.changes {
            if change
                .old_path
                .as_ref()
                .is_some_and(|path| self.conflict_paths.contains(path))
                || change
                    .new_path
                    .as_ref()
                    .is_some_and(|path| self.conflict_paths.contains(path))
            {
                continue;
            }
            let Some(path) = change.new_path.as_deref().or(change.old_path.as_deref()) else {
                continue;
            };
            let Some(kind) = associated_kind(path) else {
                continue;
            };
            if scanned == ASSOCIATED_CHANGE_SCAN_LIMIT || self.remaining_changes == 0 {
                truncated = true;
                break;
            }
            scanned += 1;
            self.remaining_changes -= 1;
            let patch = self.session.patch_history_for_change(
                &commit.oid,
                change.ordinal,
                ASSOCIATED_HUNK_SCAN_LIMIT,
                ASSOCIATED_HUNK_BYTE_LIMIT,
            )?;
            let mut patch_truncated = patch.truncated || patch.missing_objects;
            let (hunks, shared) =
                matching_material_hunks(patch.hunks, &conflict_signals, &mut patch_truncated);
            if shared.is_empty() {
                truncated |= patch_truncated;
                continue;
            }
            let key = (commit.oid.clone(), change.ordinal);
            let existing = self.material_indices.get(&key).copied();
            if existing.is_none() && material_ids.len() == ASSOCIATED_MATERIALS_PER_LEAD {
                truncated = true;
                break;
            }
            if existing.is_none() && self.materials.len() == ASSOCIATED_MATERIAL_LIMIT {
                truncated = true;
                break;
            }
            let index = if let Some(index) = existing {
                index
            } else {
                let index = self.materials.len();
                self.materials.push(AssociatedMaterial {
                    id: index + 1,
                    commit: commit.oid.clone(),
                    path: String::from_utf8_lossy(path).into_owned(),
                    path_bytes: path.to_vec(),
                    kind,
                    association: "same-commit changes share exact distinctive identities; historical association, not proof of dependency",
                    shared_identities: Vec::new(),
                    hunks: Vec::new(),
                    hunks_truncated: false,
                    associated_with: Vec::new(),
                    associations_truncated: false,
                });
                self.material_indices.insert(key, index);
                index
            };
            let material = &mut self.materials[index];
            merge_sorted_unique(&mut material.shared_identities, shared.iter().cloned());
            let excerpts_truncated = hunks.iter().any(|hunk| hunk.excerpt_truncated);
            let mut hunks_truncated = patch_truncated;
            merge_material_hunks(&mut material.hunks, hunks, &mut hunks_truncated);
            material.hunks_truncated |= hunks_truncated || excerpts_truncated;

            let reference = AssociatedLead {
                conflict_path: String::from_utf8_lossy(conflict_path).into_owned(),
                side,
                lead_commit: commit.oid.clone(),
                shared_identities: shared.into_iter().collect(),
            };
            if !material.associated_with.iter().any(|existing| {
                existing.conflict_path == reference.conflict_path
                    && existing.side == reference.side
                    && existing.lead_commit == reference.lead_commit
            }) {
                if material.associated_with.len() == FILE_LIMIT * 2 {
                    material.associations_truncated = true;
                    truncated = true;
                } else {
                    material.associated_with.push(reference);
                }
            }
            if !material_ids.contains(&material.id) {
                material_ids.push(material.id);
            }
        }
        self.truncated |= truncated;
        Ok((material_ids, truncated))
    }
}

pub(crate) fn execute(paths: Vec<String>, limit: usize) -> Result<Outcome, AppError> {
    let repository = git::Repository::discover()?;
    let mut target = repository.merge_conflict()?;
    let total_unmerged_paths = target.files.len();
    let unmerged_paths = target
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect::<HashSet<_>>();
    let paths: Vec<_> = paths.iter().map(|path| path.replace('\\', "/")).collect();
    for path in &paths {
        if !target.files.iter().any(|file| file.path == path.as_bytes()) {
            return Err(AppError::input(format!(
                "path {path:?} is not an unmerged path; use exact repository-relative paths"
            )));
        }
    }
    if !paths.is_empty() {
        target
            .files
            .retain(|file| paths.iter().any(|path| file.path == path.as_bytes()));
    }
    let selected_paths = target.files.len();
    let files_truncated = selected_paths > FILE_LIMIT;
    target.files.truncate(FILE_LIMIT);
    let mut progress = Vec::new();
    for endpoint in [&target.ours, &target.theirs] {
        let prepared = cache::prepare_at(&repository, endpoint.clone(), &mut |_| {})?;
        progress.extend(prepared.release().0);
    }
    let session = cache::open_query(&repository)?;
    for oid in [&target.ours, &target.theirs, &target.base] {
        session.require_revision(oid)?;
    }
    progress.extend_from_slice(session.progress());
    let mut warnings = session.warnings().to_vec();
    warnings.extend(
        progress
            .iter()
            .filter(|line| line.starts_with("warning:"))
            .cloned(),
    );
    let cached = session.commit_oids()?;
    let excluded = session.ancestors(&target.base)?;
    let mut histories = Vec::new();
    for (name, endpoint) in [("ours", &target.ours), ("theirs", &target.theirs)] {
        let mut reachable = session.ancestors(endpoint)?;
        let missing = reachable.difference(&cached).count();
        if missing > 0 {
            warnings.push(format!("{name} endpoint {endpoint}: incomplete history coverage; {missing} known ancestor(s) unavailable in the prepared cache (for example, beyond a shallow boundary)"));
        }
        reachable.retain(|oid| cached.contains(oid) && !excluded.contains(oid));
        histories.push((name, endpoint, reachable));
    }
    warnings.sort();
    warnings.dedup();
    let mut associated_materials = AssociatedMaterialCollector::new(&session, &unmerged_paths);
    let mut files = Vec::new();
    for file in target.files {
        let mut sides = Vec::new();
        if file.unsupported.is_none() {
            for (name, endpoint, reachable) in &histories {
                let history = session.path_history(&file.path, reachable)?;
                let mut leads = Vec::new();
                for commit in history {
                    let hunks = session.history_hunks(&commit.oid)?;
                    let matching: Vec<_> = hunks
                        .into_iter()
                        .filter(|hunk| {
                            commit.changes.iter().any(|change| {
                                change.ordinal == hunk.change_ordinal
                                    && (change.old_path.as_deref() == Some(file.path.as_slice())
                                        || change.new_path.as_deref() == Some(file.path.as_slice()))
                            })
                        })
                        .collect();
                    if matching.is_empty() {
                        continue;
                    }
                    let (associated_material_ids, associated_materials_truncated) =
                        if leads.len() < limit {
                            associated_materials.associate(&file.path, name, &commit, &matching)?
                        } else {
                            (Vec::new(), false)
                        };
                    let regions_truncated = matching.len() > REGION_LIMIT;
                    let regions = matching
                        .into_iter()
                        .take(REGION_LIMIT)
                        .map(|hunk| Region {
                            change_ordinal: hunk.change_ordinal,
                            hunk_ordinal: hunk.hunk_ordinal,
                            old_start: hunk.old_start,
                            old_lines: hunk.old_lines,
                            new_start: hunk.new_start,
                            new_lines: hunk.new_lines,
                        })
                        .collect();
                    let (subject, subject_truncated) = bounded(&commit.subject);
                    let (reason, reason_truncated) = bounded(&commit.body);
                    let message_lossy = session
                        .commit_message(&commit.oid)?
                        .is_some_and(|message| std::str::from_utf8(&message).is_err());
                    leads.push(Lead {
                        commit: commit.oid, path: String::from_utf8_lossy(&file.path).into_owned(),
                        association: "same-path textual change after merge base (containing-file evolution; not conflict-region lineage)",
                        subject, recorded_reason: (!reason.trim().is_empty()).then_some(reason),
                        reason_source: "commit message body (recorded participant text; not inferred or verified)",
                        message_truncated: subject_truncated || reason_truncated,
                        message_lossy,
                        regions, regions_truncated,
                        associated_material_ids,
                        associated_materials_truncated,
                    });
                }
                let total_leads = leads.len();
                leads.truncate(limit);
                sides.push(Side {
                    name,
                    endpoint: (*endpoint).clone(),
                    leads,
                    total_leads,
                    truncated: total_leads > limit,
                });
            }
        }
        files.push(File {
            path: String::from_utf8_lossy(&file.path).into_owned(),
            path_bytes: file.path,
            unsupported: file.unsupported,
            sides,
        });
    }
    let coverage_complete = warnings.is_empty()
        && !files_truncated
        && files.iter().all(|file| file.unsupported.is_none());
    let report = Report {
        schema_version: 2, ours: target.ours, theirs: target.theirs, merge_base: target.base,
        total_unmerged_paths, selected_paths, files, files_truncated, coverage_complete,
        associated_materials: associated_materials.materials,
        associated_materials_truncated: associated_materials.truncated,
        limits: Limits {
            files: FILE_LIMIT,
            leads_per_file_side: limit,
            regions_per_lead: REGION_LIMIT,
            associated_materials_per_report: ASSOCIATED_MATERIAL_LIMIT,
            associated_materials_per_lead: ASSOCIATED_MATERIALS_PER_LEAD,
            associated_changes_per_report: ASSOCIATED_CHANGE_REPORT_LIMIT,
            associated_changes_per_lead: ASSOCIATED_CHANGE_SCAN_LIMIT,
            associated_hunks_per_material: ASSOCIATED_HUNK_LIMIT,
            associated_lines_per_hunk: ASSOCIATED_LINE_LIMIT,
            associated_line_characters: ASSOCIATED_LINE_CHAR_LIMIT,
            message_characters: MESSAGE_LIMIT,
        },
        limitations: vec!["Direct same-path textual changes since the merge base only; no rename lineage, older history, semantic claims or generated resolution. Cached merge changes are first-parent diffs.".to_owned(), "Associated material is selected from same-commit code, test and configuration changes by exact distinctive identities in changed lines; this historical association is not proof of dependency or a requirement that files change together.".to_owned(), "Commit message bodies are recorded participant reasons, bounded and decoded as UTF-8 with replacement for invalid bytes (message_lossy identifies affected leads); absence does not establish that no reason existed.".to_owned()],
        warnings,
    };
    Ok(Outcome {
        progress,
        warnings: Vec::new(),
        report: QueryReport::Conflicts(report),
    })
}
