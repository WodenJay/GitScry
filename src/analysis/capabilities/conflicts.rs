//! Direct cached file-change evidence, separately oriented to pinned merge sides.
use crate::analysis::{
    provenance::revert_reason,
    query::{self, Outcome, QueryReport},
    retrieval,
};
use crate::{
    app::AppError,
    cache::{self, QuerySession, SearchFilter},
    git,
};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap, HashSet};
const FILE_LIMIT: usize = 100;
const PATH_CHANGE_LIMIT: usize = 20;
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
const RELATED_HISTORY_LIMIT: usize = 3;

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
    pub(crate) historical_cases: HistoricalCases,
    pub(crate) coverage_complete: bool,
    pub(crate) limits: Limits,
    pub(crate) limitations: Vec<String>,
    pub(crate) warnings: Vec<String>,
}
#[derive(Serialize)]
pub(crate) struct Limits {
    files: usize,
    leads_per_file_side: usize,
    path_changes_per_lead: usize,
    regions_per_lead: usize,
    associated_materials_per_report: usize,
    associated_materials_per_lead: usize,
    associated_changes_per_report: usize,
    associated_changes_per_lead: usize,
    associated_hunks_per_material: usize,
    associated_lines_per_hunk: usize,
    associated_line_characters: usize,
    message_characters: usize,
    related_history_per_file: usize,
    historical_cases_per_file: usize,
}
#[derive(Serialize)]
pub(crate) struct File {
    pub(crate) path: String,
    pub(crate) path_bytes: Vec<u8>,
    pub(crate) file_level: bool,
    pub(crate) index_stages: Vec<u8>,
    pub(crate) unsupported: Option<String>,
    pub(crate) sides: Vec<Side>,
    pub(crate) related_history: Vec<RelatedLead>,
    pub(crate) related_history_total: usize,
    pub(crate) related_history_truncated: bool,
}
#[derive(Serialize)]
pub(crate) struct Side {
    pub(crate) name: &'static str,
    pub(crate) endpoint: String,
    pub(crate) index_stage: u8,
    pub(crate) index_stage_present: bool,
    pub(crate) available_paths: Vec<String>,
    pub(crate) available_path_bytes: Vec<Vec<u8>>,
    pub(crate) leads: Vec<Lead>,
    pub(crate) total_leads: usize,
    pub(crate) truncated: bool,
}
#[derive(Serialize)]
pub(crate) struct Lead {
    pub(crate) commit: String,
    pub(crate) side: &'static str,
    pub(crate) conflict_path: String,
    pub(crate) association: &'static str,
    pub(crate) selection_basis: &'static str,
    pub(crate) subject: String,
    pub(crate) recorded_reason: Option<String>,
    pub(crate) reason_source: &'static str,
    pub(crate) message_truncated: bool,
    pub(crate) message_lossy: bool,
    pub(crate) path_changes: Vec<PathReference>,
    pub(crate) path_changes_truncated: bool,
    pub(crate) regions: Vec<Region>,
    pub(crate) regions_truncated: bool,
    pub(crate) associated_material_ids: Vec<usize>,
    pub(crate) associated_materials_truncated: bool,
}
#[derive(Serialize)]
pub(crate) struct PathReference {
    pub(crate) change_ordinal: i64,
    pub(crate) status: String,
    pub(crate) old_path: Option<String>,
    pub(crate) new_path: Option<String>,
}
#[derive(Serialize)]
pub(crate) struct RelatedLead {
    pub(crate) commit: String,
    pub(crate) path: String,
    pub(crate) association: &'static str,
    pub(crate) related_sides: [&'static str; 2],
    pub(crate) selection_basis: Vec<&'static str>,
    pub(crate) subject: String,
    pub(crate) recorded_reason: Option<String>,
    pub(crate) reason_source: &'static str,
    pub(crate) reverted_by: Option<HistoricalReference>,
    pub(crate) reverts_commit: Option<String>,
    pub(crate) message_truncated: bool,
    pub(crate) message_lossy: bool,
    pub(crate) regions: Vec<Region>,
    pub(crate) regions_truncated: bool,
}
#[derive(Serialize)]
pub(crate) struct HistoricalReference {
    pub(crate) commit: String,
    pub(crate) subject: String,
}
#[derive(Serialize)]
pub(crate) struct Region {
    pub(crate) change_ordinal: i64,
    pub(crate) hunk_ordinal: i64,
    pub(crate) old_path: Option<String>,
    pub(crate) new_path: Option<String>,
    pub(crate) old_start: i64,
    pub(crate) old_lines: i64,
    pub(crate) new_start: i64,
    pub(crate) new_lines: i64,
}

#[derive(Serialize)]
pub(crate) struct HistoricalCases {
    pub(crate) status: &'static str,
    pub(crate) git_version: String,
    pub(crate) reconstruction_rules: &'static str,
    pub(crate) reasons: Vec<String>,
    pub(crate) files_analyzed: usize,
    pub(crate) files_with_cases: usize,
    pub(crate) files: Vec<HistoricalFile>,
}

#[derive(Serialize)]
pub(crate) struct HistoricalFile {
    pub(crate) path: String,
    pub(crate) status: &'static str,
    pub(crate) candidate_merges: usize,
    pub(crate) candidates_examined: usize,
    pub(crate) candidates_skipped: usize,
    pub(crate) reasons: Vec<String>,
    pub(crate) cases: Vec<HistoricalCase>,
    pub(crate) cases_total: usize,
    pub(crate) cases_truncated: bool,
}

#[derive(Serialize)]
pub(crate) struct HistoricalCase {
    pub(crate) merge_commit: String,
    pub(crate) merge_base: HistoricalObject,
    pub(crate) parents: [HistoricalObject; 2],
    pub(crate) related_sides: Vec<&'static str>,
    pub(crate) reconstructed_conflict: HistoricalObject,
    pub(crate) result: HistoricalObject,
    pub(crate) association: &'static str,
    pub(crate) region_trace: RegionTrace,
}

#[derive(Serialize)]
pub(crate) struct RegionTrace {
    pub(crate) current_start_line: usize,
    pub(crate) current_lines: usize,
    pub(crate) historical_start_line: usize,
    pub(crate) historical_lines: usize,
}

#[derive(Serialize)]
pub(crate) struct HistoricalObject {
    pub(crate) commit: Option<String>,
    pub(crate) path: String,
    pub(crate) blob: String,
    pub(crate) excerpt: String,
    pub(crate) excerpt_truncated: bool,
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

fn change_belongs_to_incarnation(
    change: &cache::PathChange,
    incarnations: &cache::FileIncarnationHistory,
    revision: &str,
    incarnation: Option<&cache::FileIncarnation>,
    conflict_path: &[u8],
) -> bool {
    if let Some(incarnation) = incarnation {
        incarnations.change_has_identity(revision, change.ordinal, incarnation)
    } else {
        change.old_path.as_deref() == Some(conflict_path)
            || change.new_path.as_deref() == Some(conflict_path)
    }
}

fn incarnation_paths(
    incarnations: &cache::FileIncarnationHistory,
    endpoint_paths: &BTreeSet<Vec<u8>>,
    incarnation: Option<&cache::FileIncarnation>,
    conflict_path: &[u8],
) -> Vec<Vec<u8>> {
    let mut paths = if let Some(incarnation) = incarnation {
        incarnations.aliases_in(incarnation, endpoint_paths)
    } else {
        Vec::new()
    };
    if endpoint_paths.contains(conflict_path) && !paths.iter().any(|path| path == conflict_path) {
        paths.push(conflict_path.to_vec());
    }
    paths.sort();
    paths.dedup();
    paths
}

fn history_anchor(
    conflict_path: &[u8],
    incarnation: Option<&cache::FileIncarnation>,
    available_paths: &[Vec<u8>],
    incarnations: &cache::FileIncarnationHistory,
    base_paths: &BTreeSet<Vec<u8>>,
    endpoint_paths: &BTreeSet<Vec<u8>>,
) -> Option<Vec<u8>> {
    if endpoint_paths.contains(conflict_path) {
        return Some(conflict_path.to_vec());
    }
    if let Some(path) = available_paths.first() {
        return Some(path.clone());
    }
    incarnation
        .and_then(|incarnation| {
            incarnations
                .aliases_in(incarnation, base_paths)
                .into_iter()
                .next()
                .or_else(|| Some(incarnation.path.clone()))
        })
        .or_else(|| {
            base_paths
                .contains(conflict_path)
                .then(|| conflict_path.to_vec())
        })
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
    let session =
        cache::refresh_query_targets(&repository, &[target.ours.as_str(), target.theirs.as_str()])?;
    for oid in [&target.ours, &target.theirs, &target.base] {
        session.require_revision(oid)?;
    }
    let progress = session.progress().to_vec();
    let mut warnings = session.warnings().to_vec();
    warnings.extend(
        progress
            .iter()
            .filter(|line| line.starts_with("warning:"))
            .cloned(),
    );
    let cached = session.commit_oids()?;
    let excluded = session.ancestors(&target.base)?;
    let current_paths = target
        .files
        .iter()
        .filter(|file| file.unsupported.is_none())
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    let target_revisions = [target.ours.clone(), target.theirs.clone()];
    let incarnations = session.file_incarnations(&target_revisions, &current_paths)?;
    let base_paths = repository.file_paths_at(&target.base)?;
    let mut histories = Vec::new();
    let mut history_incomplete = false;
    for (name, endpoint) in [("ours", &target.ours), ("theirs", &target.theirs)] {
        let mut reachable = session.ancestors(endpoint)?;
        let missing = reachable.difference(&cached).count();
        if missing > 0 {
            history_incomplete = true;
            warnings.push(format!("{name} endpoint {endpoint}: incomplete history coverage; {missing} known ancestor(s) unavailable in the prepared cache (for example, beyond a shallow boundary)"));
        }
        reachable.retain(|oid| cached.contains(oid) && !excluded.contains(oid));
        let endpoint_paths = repository.file_paths_at(endpoint)?;
        histories.push((name, endpoint, reachable, endpoint_paths));
    }
    let mut historical_cases =
        super::historical_conflicts::analyze(&repository, &target, &cached, &incarnations);
    if files_truncated {
        historical_cases.status = "partial";
        historical_cases
            .reasons
            .push("the selected conflict files exceed the 100-file query limit".to_owned());
    }
    if history_incomplete {
        historical_cases.status = "partial";
        historical_cases
            .reasons
            .push("one or both side histories are incomplete in the prepared cache".to_owned());
    }
    let mut associated_materials = AssociatedMaterialCollector::new(&session, &unmerged_paths);
    let shared_reachable = excluded
        .intersection(&cached)
        .cloned()
        .collect::<HashSet<_>>();
    let shared_revisions = shared_reachable.iter().cloned().collect::<Vec<_>>();
    let shared_scope =
        query::scope::install_shared_history_scope(&session, &shared_revisions, &target.base)?;
    let reverts = retrieval::reverts(&session, Some(&shared_scope))?;
    let mut files = Vec::new();
    for file in target.files {
        let mut sides = Vec::new();
        if file.unsupported.is_none() {
            for (name, endpoint, reachable, endpoint_paths) in &histories {
                let (index_stage, counterpart) = match *name {
                    "ours" => (2, &target.theirs),
                    "theirs" => (3, &target.ours),
                    _ => unreachable!("conflict history has exactly two sides"),
                };
                let own_incarnation = incarnations.identity_at(endpoint, &file.path);
                let counterpart_incarnation = incarnations.identity_at(counterpart, &file.path);
                let incarnation = if own_incarnation.is_some() {
                    own_incarnation
                } else if !endpoint_paths.contains(&file.path) {
                    counterpart_incarnation
                } else {
                    None
                };
                if incarnation.is_none() {
                    warnings.push(format!(
                        "{name} endpoint {endpoint}: no file incarnation established for conflict path {:?}; only exact-path changes can be shown when available",
                        String::from_utf8_lossy(&file.path)
                    ));
                }
                let available_path_bytes =
                    incarnation_paths(&incarnations, endpoint_paths, incarnation, &file.path);
                let available_paths = available_path_bytes
                    .iter()
                    .map(|path| String::from_utf8_lossy(path).into_owned())
                    .collect::<Vec<_>>();
                let index_stage_present = file.index_stages.contains(&index_stage);
                let history_path = history_anchor(
                    &file.path,
                    incarnation,
                    &available_path_bytes,
                    &incarnations,
                    &base_paths,
                    endpoint_paths,
                );
                let history = if let Some(path) = history_path {
                    session.path_history(&path, reachable)?
                } else {
                    Vec::new()
                };
                let mut leads = Vec::new();
                for commit in history {
                    let associated_changes = commit
                        .changes
                        .iter()
                        .filter(|change| {
                            change_belongs_to_incarnation(
                                change,
                                &incarnations,
                                &commit.oid,
                                incarnation,
                                &file.path,
                            )
                        })
                        .collect::<Vec<_>>();
                    if associated_changes.is_empty() {
                        continue;
                    }
                    let touches_conflict_path = associated_changes.iter().any(|change| {
                        change.old_path.as_deref() == Some(file.path.as_slice())
                            || change.new_path.as_deref() == Some(file.path.as_slice())
                    });
                    let selection_basis = if incarnation.is_none() {
                        "conflict path (file incarnation unestablished)"
                    } else if touches_conflict_path {
                        "conflict path"
                    } else {
                        "detected rename lineage"
                    };
                    let path_changes_truncated = associated_changes.len() > PATH_CHANGE_LIMIT;
                    let path_changes = associated_changes
                        .iter()
                        .take(PATH_CHANGE_LIMIT)
                        .map(|change| PathReference {
                            change_ordinal: change.ordinal,
                            status: change.status.clone(),
                            old_path: change
                                .old_path
                                .as_ref()
                                .map(|path| String::from_utf8_lossy(path).into_owned()),
                            new_path: change
                                .new_path
                                .as_ref()
                                .map(|path| String::from_utf8_lossy(path).into_owned()),
                        })
                        .collect();
                    let hunks = session.history_hunks(&commit.oid)?;
                    let matching = hunks
                        .into_iter()
                        .filter(|hunk| {
                            associated_changes
                                .iter()
                                .any(|change| change.ordinal == hunk.change_ordinal)
                        })
                        .collect::<Vec<_>>();
                    let (associated_material_ids, associated_materials_truncated) =
                        if leads.len() < limit {
                            associated_materials.associate(&file.path, name, &commit, &matching)?
                        } else {
                            (Vec::new(), false)
                        };
                    let regions_truncated = matching.len() > REGION_LIMIT;
                    let regions = matching
                        .iter()
                        .take(REGION_LIMIT)
                        .map(|hunk| {
                            let change = associated_changes
                                .iter()
                                .find(|change| change.ordinal == hunk.change_ordinal)
                                .unwrap();
                            Region {
                                change_ordinal: hunk.change_ordinal,
                                hunk_ordinal: hunk.hunk_ordinal,
                                old_path: change
                                    .old_path
                                    .as_ref()
                                    .map(|path| String::from_utf8_lossy(path).into_owned()),
                                new_path: change
                                    .new_path
                                    .as_ref()
                                    .map(|path| String::from_utf8_lossy(path).into_owned()),
                                old_start: hunk.old_start,
                                old_lines: hunk.old_lines,
                                new_start: hunk.new_start,
                                new_lines: hunk.new_lines,
                            }
                        })
                        .collect();
                    let (subject, subject_truncated) = bounded(&commit.subject);
                    let (reason, reason_truncated) = bounded(&commit.body);
                    let message_lossy = session
                        .commit_message(&commit.oid)?
                        .is_some_and(|message| std::str::from_utf8(&message).is_err());
                    leads.push(Lead {
                        commit: commit.oid,
                        side: name,
                        conflict_path: String::from_utf8_lossy(&file.path).into_owned(),
                        association: if incarnation.is_some() {
                            "same file incarnation in side history; no semantic responsibility or conflict-region lineage asserted"
                        } else {
                            "exact conflict-path change only; file incarnation unestablished"
                        },
                        selection_basis,
                        subject,
                        recorded_reason: (!reason.trim().is_empty()).then_some(reason),
                        reason_source: "commit message body (recorded participant text; not inferred or verified)",
                        message_truncated: subject_truncated || reason_truncated,
                        message_lossy,
                        path_changes,
                        path_changes_truncated,
                        regions,
                        regions_truncated,
                        associated_material_ids,
                        associated_materials_truncated,
                    });
                }
                let total_leads = leads.len();
                leads.truncate(limit);
                sides.push(Side {
                    name,
                    endpoint: (*endpoint).clone(),
                    index_stage,
                    index_stage_present,
                    available_paths,
                    available_path_bytes,
                    leads,
                    total_leads,
                    truncated: total_leads > limit,
                });
            }
        }

        let (related_history, related_history_total, related_history_truncated) =
            if file.unsupported.is_none() {
                related_history(
                    &session,
                    &file.path,
                    &shared_reachable,
                    &shared_scope,
                    &reverts,
                    RELATED_HISTORY_LIMIT,
                )?
            } else {
                (Vec::new(), 0, false)
            };
        files.push(File {
            path: String::from_utf8_lossy(&file.path).into_owned(),
            path_bytes: file.path,
            file_level: file.file_level,
            index_stages: file.index_stages,
            unsupported: file.unsupported,
            sides,
            related_history,
            related_history_total,
            related_history_truncated,
        });
    }
    warnings.sort();
    warnings.dedup();
    let coverage_complete = warnings.is_empty()
        && !files_truncated
        && files.iter().all(|file| file.unsupported.is_none());
    let report = Report {
        schema_version: 5,
        ours: target.ours,
        theirs: target.theirs,
        merge_base: target.base,
        total_unmerged_paths,
        selected_paths,
        files,
        files_truncated,
        associated_materials: associated_materials.materials,
        associated_materials_truncated: associated_materials.truncated,
        historical_cases,
        coverage_complete,
        limits: Limits {
            files: FILE_LIMIT,
            leads_per_file_side: limit,
            path_changes_per_lead: PATH_CHANGE_LIMIT,
            regions_per_lead: REGION_LIMIT,
            associated_materials_per_report: ASSOCIATED_MATERIAL_LIMIT,
            associated_materials_per_lead: ASSOCIATED_MATERIALS_PER_LEAD,
            associated_changes_per_report: ASSOCIATED_CHANGE_REPORT_LIMIT,
            associated_changes_per_lead: ASSOCIATED_CHANGE_SCAN_LIMIT,
            associated_hunks_per_material: ASSOCIATED_HUNK_LIMIT,
            associated_lines_per_hunk: ASSOCIATED_LINE_LIMIT,
            associated_line_characters: ASSOCIATED_LINE_CHAR_LIMIT,
            message_characters: MESSAGE_LIMIT,
            related_history_per_file: RELATED_HISTORY_LIMIT,
            historical_cases_per_file: super::historical_conflicts::CASE_LIMIT_PER_FILE,
        },
        limitations: vec![
            "History is bounded to each side's commits after the merge base and selected by file-incarnation identity; detected renames are followed. Cross-file migration, semantic responsibility, and paths without an established incarnation are not inferred. Cached merge changes are first-parent diffs."
                .to_owned(),
            "Associated material is selected from same-commit code, test and configuration changes by exact distinctive identities in changed lines; this historical association is not proof of dependency or a requirement that files change together."
                .to_owned(),
            "Shared history is limited to same-path textual changes in cached merge-base ancestry shared by both pinned endpoints; these are contextual precedents, not causal explanations. Rename lineage and associated changes are not included in this section."
                .to_owned(),
            "Commit message bodies and revert reasons are recorded participant text, bounded and decoded as UTF-8 with replacement for invalid bytes (message_lossy identifies affected leads); absence does not establish that no reason existed."
                .to_owned(),
        ],
        warnings,
    };
    Ok(Outcome {
        progress,
        warnings: Vec::new(),
        report: QueryReport::Conflicts(report),
    })
}

fn related_history(
    session: &QuerySession,
    path: &[u8],
    reachable: &HashSet<String>,
    scope: &SearchFilter,
    reverts: &retrieval::RevertIndex,
    limit: usize,
) -> Result<(Vec<RelatedLead>, usize, bool), AppError> {
    let history = session
        .path_history(path, reachable)?
        .into_iter()
        .filter(|commit| {
            commit.changes.iter().any(|change| {
                change.old_path.as_deref() == Some(path) || change.new_path.as_deref() == Some(path)
            })
        })
        .collect::<Vec<_>>();
    let history_oids = history
        .iter()
        .map(|commit| commit.oid.clone())
        .collect::<HashSet<_>>();
    let mut paired_reverts = HashSet::new();
    let mut linked_reverts = HashMap::new();
    for commit in &history {
        if let Some(revert) = retrieval::link_change(
            session,
            reverts,
            Some(scope),
            &commit.oid,
            &commit.paths,
            commit.position,
        )? {
            if history_oids.contains(&revert.oid) {
                paired_reverts.insert(revert.oid.clone());
            }
            linked_reverts.insert(commit.oid.clone(), revert);
        }
    }

    let total = history.len().saturating_sub(paired_reverts.len());
    let mut candidates = history
        .into_iter()
        .filter(|commit| !paired_reverts.contains(&commit.oid))
        .collect::<Vec<_>>();
    // Put explicit revert links first; keep recency within each priority.
    candidates.sort_by_key(|commit| {
        !(linked_reverts.contains_key(&commit.oid) || reverts.is_revert(&commit.oid))
    });
    let mut leads = Vec::new();
    for commit in candidates.into_iter().take(limit) {
        let linked_revert = linked_reverts.get(&commit.oid).copied();
        let is_revert = reverts.is_revert(&commit.oid);
        let (subject, subject_truncated) = bounded(&commit.subject);
        let (recorded_reason, reason_truncated, reason_source) = if let Some(revert) = linked_revert
        {
            let (reason, truncated) =
                bounded_optional(revert_reason(&revert.subject, &revert.body));
            (
                reason,
                truncated,
                "recorded in the reverting commit body; not inferred",
            )
        } else if is_revert {
            let (reason, truncated) =
                bounded_optional(revert_reason(&commit.subject, &commit.body));
            (
                reason,
                truncated,
                "recorded in this revert commit body; not inferred",
            )
        } else {
            let (reason, truncated) = bounded(&commit.body);
            (
                (!reason.trim().is_empty()).then_some(reason),
                truncated,
                "commit message body (recorded participant text; not inferred or verified)",
            )
        };
        let (reverted_by, revert_subject_truncated, revert_message_lossy) =
            if let Some(revert) = linked_revert {
                let (subject, truncated) = bounded(&revert.subject);
                (
                    Some(HistoricalReference {
                        commit: revert.oid.clone(),
                        subject,
                    }),
                    truncated,
                    message_is_lossy(session, &revert.oid)?,
                )
            } else {
                (None, false, false)
            };
        let mut selection_basis = vec![
            "same conflicted path changed in cached history reachable from the merge base",
            "merge-base ancestry is shared by the pinned ours and theirs endpoints",
        ];
        if linked_revert.is_some() {
            selection_basis.push(if reverts.of(&commit.oid).is_some() {
                "revert trailer names this change"
            } else {
                "revert covers all changed paths without an intervening path touch"
            });
        } else if is_revert {
            selection_basis.push("commit subject identifies a revert or rollback");
        }
        let (regions, regions_truncated) = history_regions(session, &commit, path)?;
        leads.push(RelatedLead {
            commit: commit.oid.clone(),
            path: String::from_utf8_lossy(path).into_owned(),
            association: "shared pre-merge-base path history",
            related_sides: ["ours", "theirs"],
            selection_basis,
            subject,
            recorded_reason,
            reason_source,
            reverted_by,
            reverts_commit: reverts.target_of(&commit.oid).map(str::to_owned),
            message_truncated: subject_truncated || reason_truncated || revert_subject_truncated,
            message_lossy: message_is_lossy(session, &commit.oid)? || revert_message_lossy,
            regions,
            regions_truncated,
        });
    }
    let truncated = total > leads.len();
    Ok((leads, total, truncated))
}

fn history_regions(
    session: &QuerySession,
    commit: &cache::HistoryCommit,
    path: &[u8],
) -> Result<(Vec<Region>, bool), AppError> {
    let matching = session
        .history_hunks(&commit.oid)?
        .into_iter()
        .filter(|hunk| {
            commit.changes.iter().any(|change| {
                change.ordinal == hunk.change_ordinal
                    && (change.old_path.as_deref() == Some(path)
                        || change.new_path.as_deref() == Some(path))
            })
        })
        .collect::<Vec<_>>();
    let truncated = matching.len() > REGION_LIMIT;
    let regions = matching
        .into_iter()
        .take(REGION_LIMIT)
        .map(|hunk| {
            let change = commit
                .changes
                .iter()
                .find(|change| change.ordinal == hunk.change_ordinal)
                .expect("history hunk must belong to a commit change");
            Region {
                change_ordinal: hunk.change_ordinal,
                hunk_ordinal: hunk.hunk_ordinal,
                old_path: change
                    .old_path
                    .as_ref()
                    .map(|path| String::from_utf8_lossy(path).into_owned()),
                new_path: change
                    .new_path
                    .as_ref()
                    .map(|path| String::from_utf8_lossy(path).into_owned()),
                old_start: hunk.old_start,
                old_lines: hunk.old_lines,
                new_start: hunk.new_start,
                new_lines: hunk.new_lines,
            }
        })
        .collect();
    Ok((regions, truncated))
}

fn bounded_optional(text: Option<String>) -> (Option<String>, bool) {
    text.map_or((None, false), |text| {
        let (bounded, truncated) = bounded(&text);
        (Some(bounded), truncated)
    })
}

fn message_is_lossy(session: &QuerySession, oid: &str) -> Result<bool, AppError> {
    Ok(session
        .commit_message(oid)?
        .is_some_and(|message| std::str::from_utf8(&message).is_err()))
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unestablished_incarnation_only_matches_exact_path() {
        let history = cache::FileIncarnationHistory::default();
        let changes = [
            cache::PathChange {
                ordinal: 0,
                status: "R100".to_owned(),
                old_path: Some(b"lib.rs".to_vec()),
                new_path: Some(b"score.rs".to_vec()),
                old_blob: None,
                new_blob: None,
            },
            cache::PathChange {
                ordinal: 1,
                status: "A".to_owned(),
                old_path: None,
                new_path: Some(b"lib.rs".to_vec()),
                old_blob: None,
                new_blob: None,
            },
        ];
        let associated = changes
            .iter()
            .filter(|change| {
                change_belongs_to_incarnation(change, &history, "head", None, b"score.rs")
            })
            .map(|change| change.ordinal)
            .collect::<Vec<_>>();

        assert_eq!(associated, [0]);
    }
}
