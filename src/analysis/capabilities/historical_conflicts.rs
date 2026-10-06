use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::{
    app::AppError,
    cache::{self, FileIncarnationHistory},
    git::{self, MergeTree, Replay, StageEntry},
};

use super::conflicts::{
    CandidateSummary, HistoricalCase, HistoricalCases, HistoricalFile, HistoricalObject,
    HistoricalRegion, RegionTrace,
};

pub(crate) const CASE_LIMIT_PER_FILE: usize = 5;
const EXCERPT_CHAR_LIMIT: usize = 4096;
const RECONSTRUCTION_RULES: &str = "Git merge-tree --write-tree with an explicit unique base, ort strategy and find-renames; replay uses an isolated Git directory and object store, committed .gitattributes only, merge.default=text, and built-in merge drivers only. External custom drivers and global, system, and info attributes are never used.";
const RANKING_RULES: &str = "Cases are ranked by region-association strength (a unique correspondence of both surrounding contexts outranks one, which outranks none), then by merge commit recency, with the merge commit object ID as the deterministic tie-breaker. All eligible candidates are examined; the per-file case limit only bounds returned cases.";

struct Candidate {
    oid: String,
    parents: [String; 2],
    related_sides: BTreeSet<&'static str>,
    commit_time: i64,
}

struct ConflictRegion {
    before: Vec<String>,
    after: Vec<String>,
    ours_lines: Vec<String>,
    theirs_lines: Vec<String>,
    start_line: usize,
    line_count: usize,
}

enum RegionMatch {
    /// A unique correspondence was found; carries the current and historical
    /// region indices plus the explainable association basis.
    Unique {
        current: usize,
        historical: usize,
        basis: &'static str,
    },
    /// The historical conflict exists but no unique correspondence exists;
    /// the case is retained without region mapping.
    None,
    /// Correspondences exist but none is unique, so no precise mapping
    /// exists; the case is retained without region mapping.
    Ambiguous,
}

enum PathMatch {
    Missing,
    Unique(String),
    Ambiguous,
}

#[derive(Default)]
struct CandidateAccounting {
    discovered: usize,
    examined: usize,
    nonconflicting: usize,
    irrelevant: usize,
    ambiguous: usize,
    unsupported: usize,
    failed: usize,
}

/// Tracks the optional historical-examination execution budget. The budget
/// bounds replay checks only; candidate discovery, existing side-history
/// leads, cache refresh, and output limits are never governed by it.
struct Budget {
    remaining: Option<usize>,
    exhausted: bool,
}

impl Budget {
    fn new(budget: Option<usize>) -> Self {
        Self {
            remaining: budget,
            exhausted: false,
        }
    }

    /// Reserve one replay check. Returns false when the budget is spent;
    /// the caller must stop launching work without examining anything.
    fn start_check(&mut self) -> bool {
        match self.remaining {
            Some(0) => {
                self.exhausted = true;
                false
            }
            Some(remaining) => {
                self.remaining = Some(remaining - 1);
                true
            }
            None => true,
        }
    }
}

impl CandidateAccounting {
    fn discovered(count: usize) -> Self {
        Self {
            discovered: count,
            ..Self::default()
        }
    }

    fn examine(&mut self, outcome: CandidateOutcome) {
        self.examined += 1;
        match outcome {
            CandidateOutcome::NonConflicting => self.nonconflicting += 1,
            CandidateOutcome::Irrelevant => self.irrelevant += 1,
            CandidateOutcome::Ambiguous => self.ambiguous += 1,
            CandidateOutcome::Unsupported => self.unsupported += 1,
            CandidateOutcome::Failed => self.failed += 1,
        }
    }

    fn unchecked(&self) -> usize {
        self.discovered.saturating_sub(self.examined)
    }

    fn project(&self, report: &mut HistoricalFile) {
        report.candidate_merges = self.discovered;
        report.candidates_examined = self.examined;
        report.candidates_nonconflicting = self.nonconflicting;
        report.candidates_irrelevant = self.irrelevant;
        report.candidates_ambiguous = self.ambiguous;
        report.candidates_unsupported = self.unsupported;
        report.candidates_failed = self.failed;
        report.candidates_unchecked = self.unchecked();
    }

    fn merge(&mut self, other: &CandidateAccounting) {
        self.examined += other.examined;
        self.nonconflicting += other.nonconflicting;
        self.irrelevant += other.irrelevant;
        self.ambiguous += other.ambiguous;
        self.unsupported += other.unsupported;
        self.failed += other.failed;
    }
}

enum CandidateOutcome {
    NonConflicting,
    Irrelevant,
    Ambiguous,
    Unsupported,
    Failed,
}

pub(crate) fn analyze(
    repository: &git::Repository,
    target: &git::MergeConflict,
    cached: &HashSet<String>,
    incarnations: &FileIncarnationHistory,
    budget: Option<usize>,
) -> HistoricalCases {
    let runner = match MergeTree::new(repository) {
        Ok(runner) => runner,
        Err(error) => return unavailable("unknown".to_owned(), error.to_string()),
    };
    let version = runner.version().to_owned();
    if !runner.supported() {
        return unavailable(
            version.clone(),
            format!("Git 2.40 or newer is required; found {version}"),
        );
    }
    match analyze_with_runner(&runner, repository, target, cached, incarnations, budget) {
        Ok((mut files, accounting, discovery_complete, budget_exhausted)) => {
            let files_analyzed = files.len();
            let files_with_cases = files.iter().filter(|file| file.cases_total > 0).count();
            let mut status = if files.iter().any(|file| file.status != "complete") {
                "partial"
            } else {
                "complete"
            };
            let mut reasons = Vec::new();
            if accounting.failed > 0 {
                status = "partial";
                reasons.push(format!(
                    "{} historical candidate(s) could not be reconstructed",
                    accounting.failed
                ));
            }
            if accounting.unchecked() > 0 {
                status = "partial";
                reasons.push(format!(
                    "{} historical candidate(s) were not examined",
                    accounting.unchecked()
                ));
            }
            if !discovery_complete {
                status = "partial";
                reasons.push(
                    "candidate discovery did not finish; remaining candidates are unknown"
                        .to_owned(),
                );
            }
            if budget_exhausted {
                status = "partial";
                reasons.push(
                    "the historical examination budget expired before all discovered candidates were examined"
                        .to_owned(),
                );
            }
            let mut skipped_reasons = BTreeMap::<String, Vec<&str>>::new();
            for file in files
                .iter()
                .filter(|file| file.status != "complete" && file.cases_total == 0)
            {
                for reason in &file.reasons {
                    skipped_reasons
                        .entry(reason.clone())
                        .or_default()
                        .push(&file.path);
                }
            }
            for (reason, paths) in skipped_reasons {
                if paths.len() == 1 {
                    reasons.push(format!("{}: {reason}", paths[0]));
                } else {
                    reasons.push(format!("{} files: {reason}", paths.len()));
                }
            }
            files.retain(|file| file.cases_total > 0);
            let candidates = CandidateSummary {
                discovered: accounting.discovered,
                examined: accounting.examined,
                nonconflicting: accounting.nonconflicting,
                irrelevant: accounting.irrelevant,
                ambiguous: accounting.ambiguous,
                unsupported: accounting.unsupported,
                failed: accounting.failed,
                unchecked: accounting.unchecked(),
                budget,
                limited: budget_exhausted,
            };
            HistoricalCases {
                status,
                git_version: version,
                reconstruction_rules: RECONSTRUCTION_RULES,
                ranking: RANKING_RULES,
                reasons,
                candidates,
                files_analyzed,
                files_with_cases,
                files,
            }
        }
        Err(error) => unavailable(version, error.to_string()),
    }
}

fn unavailable(version: String, reason: String) -> HistoricalCases {
    HistoricalCases {
        status: "unavailable",
        git_version: version,
        reconstruction_rules: RECONSTRUCTION_RULES,
        ranking: RANKING_RULES,
        reasons: vec![reason],
        candidates: CandidateSummary::default(),
        files_analyzed: 0,
        files_with_cases: 0,
        files: Vec::new(),
    }
}

fn analyze_with_runner(
    runner: &MergeTree,
    repository: &git::Repository,
    target: &git::MergeConflict,
    cached: &HashSet<String>,
    incarnations: &FileIncarnationHistory,
    budget: Option<usize>,
) -> Result<(Vec<HistoricalFile>, CandidateAccounting, bool, bool), AppError> {
    // Discovery itself stays complete: the candidate set is computed up front
    // from the cached graph without replay work, so an expired budget leaves
    // the discovered total known and only bounds examination.
    let candidates = discover_candidates(repository, target, cached)?;
    let discovery_complete = true;
    let mut budget = Budget::new(budget);
    let current = runner.replay(&target.base, &target.ours, &target.theirs)?;
    if !current.conflicted {
        return Err(AppError::operational(
            "error: isolated replay did not reproduce the pinned conflict",
        ));
    }
    let current_attributes_unsupported = runner.has_unsupported_attributes(&[
        &target.base,
        &target.ours,
        &target.theirs,
        &current.tree,
    ])?;
    if current_attributes_unsupported {
        let mut files = target
            .files
            .iter()
            .map(|file| HistoricalFile {
                path: String::from_utf8_lossy(&file.path).into_owned(),
                status: "partial",
                candidate_merges: candidates.len(),
                candidates_examined: 0,
                candidates_nonconflicting: 0,
                candidates_irrelevant: 0,
                candidates_ambiguous: 0,
                candidates_unsupported: 0,
                candidates_failed: 0,
                candidates_unchecked: candidates.len(),
                reasons: vec![
                    "custom merge attributes occur in the current replay trees; external drivers are disabled".to_owned(),
                ],
                cases: Vec::new(),
                cases_total: 0,
                cases_omitted: 0,
                cases_truncated: false,
            })
            .collect::<Vec<_>>();
        let mut accounting = CandidateAccounting::discovered(candidates.len() * files.len());
        for file in &mut files {
            let mut per_file = CandidateAccounting::discovered(candidates.len());
            for _ in 0..candidates.len() {
                per_file.examine(CandidateOutcome::Unsupported);
            }
            per_file.project(file);
            accounting.merge(&per_file);
        }
        return Ok((files, accounting, discovery_complete, false));
    }
    let requested_paths = target
        .files
        .iter()
        .flat_map(|file| {
            let mut paths = vec![file.path.clone()];
            for revision in [&target.ours, &target.theirs] {
                if let Some(identity) = incarnations.identity_at(revision, &file.path) {
                    paths.extend(incarnations.aliases_for(identity));
                }
            }
            paths
        })
        .filter_map(|path| String::from_utf8(path).ok())
        .collect::<BTreeSet<_>>();
    let mut file_analysis = FileAnalysis {
        runner,
        repository,
        current: &current,
        candidates: &candidates,
        incarnations,
        tree_paths: HashMap::new(),
        requested_paths,
        metadata: HashMap::new(),
        ours: &target.ours,
        theirs: &target.theirs,
        budget: &mut budget,
    };
    let mut total = CandidateAccounting::discovered(candidates.len() * target.files.len());
    let files = target
        .files
        .iter()
        .map(|file| {
            file_analysis.analyze(file).map(|(mut report, accounting)| {
                total.merge(&accounting);
                accounting.project(&mut report);
                report
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((files, total, discovery_complete, budget.exhausted))
}

fn discover_candidates(
    repository: &git::Repository,
    target: &git::MergeConflict,
    cached: &HashSet<String>,
) -> Result<Vec<Candidate>, AppError> {
    let mut candidates = HashMap::<String, Candidate>::new();
    for (side, endpoint) in [
        ("ours", target.ours.as_str()),
        ("theirs", target.theirs.as_str()),
    ] {
        let graph = repository.reachable_commit_parents(endpoint)?;
        let times = repository.reachable_commit_times(endpoint)?;
        for (oid, parents) in graph {
            if parents.len() != 2 || !cached.contains(&oid) {
                continue;
            }
            let candidate = candidates.entry(oid.clone()).or_insert_with(|| Candidate {
                oid: oid.clone(),
                parents: [parents[0].clone(), parents[1].clone()],
                related_sides: BTreeSet::new(),
                commit_time: times.get(&oid).copied().unwrap_or_default(),
            });
            candidate.related_sides.insert(side);
        }
    }
    let mut candidates = candidates.into_values().collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .commit_time
            .cmp(&left.commit_time)
            .then_with(|| left.oid.cmp(&right.oid))
    });
    Ok(candidates)
}

struct FileAnalysis<'a> {
    runner: &'a MergeTree,
    repository: &'a git::Repository,
    current: &'a Replay,
    candidates: &'a [Candidate],
    incarnations: &'a FileIncarnationHistory,
    tree_paths: HashMap<String, BTreeSet<Vec<u8>>>,
    requested_paths: BTreeSet<String>,
    metadata: HashMap<String, Vec<git::TreeEntry>>,
    ours: &'a str,
    theirs: &'a str,
    budget: &'a mut Budget,
}

impl FileAnalysis<'_> {
    fn analyze(
        &mut self,
        file: &git::ConflictFile,
    ) -> Result<(HistoricalFile, CandidateAccounting), AppError> {
        let mut accounting = CandidateAccounting::discovered(self.candidates.len());
        let report = self.analyze_inner(file, &mut accounting)?;
        Ok((report, accounting))
    }

    fn analyze_inner(
        &mut self,
        file: &git::ConflictFile,
        accounting: &mut CandidateAccounting,
    ) -> Result<HistoricalFile, AppError> {
        let runner = self.runner;
        let repository = self.repository;
        let current = self.current;
        let candidates = self.candidates;
        let incarnations = self.incarnations;
        let tree_paths = &mut self.tree_paths;
        let metadata = &mut self.metadata;
        let requested_paths = &self.requested_paths;
        let ours = self.ours;
        let theirs = self.theirs;
        let path = String::from_utf8_lossy(&file.path).into_owned();
        let mut report = HistoricalFile {
            path: path.clone(),
            status: "complete",
            candidate_merges: candidates.len(),
            candidates_examined: 0,
            candidates_nonconflicting: 0,
            candidates_irrelevant: 0,
            candidates_ambiguous: 0,
            candidates_unsupported: 0,
            candidates_failed: 0,
            candidates_unchecked: 0,
            reasons: Vec::new(),
            cases: Vec::new(),
            cases_total: 0,
            cases_omitted: 0,
            cases_truncated: false,
        };
        if file.file_level || file.unsupported.is_some() || std::str::from_utf8(&file.path).is_err()
        {
            report.status = "unsupported";
            report.reasons.push(
                "historical replay requires a supported UTF-8 regular-text conflict path"
                    .to_owned(),
            );
            return Ok(report);
        }
        let Some(ours_identity) = incarnations.identity_at(ours, &file.path) else {
            return partial_file(report, "current ours path has no cached file incarnation");
        };
        let Some(theirs_identity) = incarnations.identity_at(theirs, &file.path) else {
            return partial_file(report, "current theirs path has no cached file incarnation");
        };
        if ours_identity != theirs_identity {
            return partial_file(
                report,
                "current conflict path has different file incarnations on the two sides",
            );
        }
        let identity = ours_identity;
        let Some(current_stages) = stages_for_path(current, &file.path) else {
            return partial_file(
                report,
                "isolated current replay did not produce three text stages for this path",
            );
        };
        if !current_stages.iter().all(|stage| regular_mode(&stage.mode)) {
            report.status = "unsupported";
            report
                .reasons
                .push("current replay contains a non-regular file stage".to_owned());
            return Ok(report);
        }
        let Some(current_entry) =
            cached_tree_entry(runner, metadata, requested_paths, &current.tree, &path)?
        else {
            return partial_file(
                report,
                "isolated current replay has no merged blob for this path",
            );
        };
        if !regular_mode(&current_entry.mode) {
            report.status = "unsupported";
            report
                .reasons
                .push("current replay result is not a regular file".to_owned());
            return Ok(report);
        }
        let Some(current_text) = runner.blob(&current_entry.oid)? else {
            report.status = "unsupported";
            report
                .reasons
                .push("current replay result is not bounded UTF-8 text".to_owned());
            return Ok(report);
        };
        let current_text = std::str::from_utf8(&current_text).expect("blob reader validated UTF-8");
        let Some(current_regions) = conflict_regions(current_text) else {
            return partial_file(
                report,
                "current replay conflict markers could not be parsed",
            );
        };

        let mut staged_cases = Vec::new();

        for candidate in candidates {
            // Stop launching work when the budget is spent; already found
            // cases and existing leads are retained and the report is marked
            // partial with unchecked candidates counted.
            if !self.budget.start_check() {
                report.status = "partial";
                report.reasons.push(format!(
                    "historical examination stopped after {} candidate(s) by the budget; unexamined candidates are counted but their outcomes are unknown",
                    accounting.examined
                ));
                break;
            }
            let bases = runner.merge_bases(&candidate.parents[0], &candidate.parents[1])?;
            if bases.len() != 1 {
                skip_candidate(
                    &mut report,
                    accounting,
                    CandidateOutcome::Ambiguous,
                    format!(
                        "{} has {} best merge bases; exactly one is required",
                        candidate.oid,
                        bases.len()
                    ),
                );
                continue;
            }
            let base = &bases[0];
            let paths = (
                incarnation_path_at(repository, tree_paths, incarnations, base, identity)?,
                incarnation_path_at(
                    repository,
                    tree_paths,
                    incarnations,
                    &candidate.parents[0],
                    identity,
                )?,
                incarnation_path_at(
                    repository,
                    tree_paths,
                    incarnations,
                    &candidate.parents[1],
                    identity,
                )?,
                incarnation_path_at(
                    repository,
                    tree_paths,
                    incarnations,
                    &candidate.oid,
                    identity,
                )?,
            );
            if [&paths.0, &paths.1, &paths.2, &paths.3]
                .into_iter()
                .any(|path| matches!(path, PathMatch::Ambiguous))
            {
                skip_candidate(
                    &mut report,
                    accounting,
                    CandidateOutcome::Ambiguous,
                    format!(
                        "{} has ambiguous paths for the current file incarnation",
                        candidate.oid
                    ),
                );
                continue;
            }
            let (
                PathMatch::Unique(base_path),
                PathMatch::Unique(parent1_path),
                PathMatch::Unique(parent2_path),
            ) = (paths.0, paths.1, paths.2)
            else {
                accounting.examine(CandidateOutcome::Irrelevant);
                continue;
            };
            let result_path = match paths.3 {
                PathMatch::Unique(result_path) => result_path,
                // The merge commit deleted or renamed the file; keep the case
                // with the absence disclosed instead of dropping the lead.
                PathMatch::Missing => String::new(),
                PathMatch::Ambiguous => continue,
            };
            let Some(base_entry) =
                cached_tree_entry(runner, metadata, requested_paths, base, &base_path)?
            else {
                accounting.examine(CandidateOutcome::Irrelevant);
                continue;
            };
            let Some(parent1_entry) = cached_tree_entry(
                runner,
                metadata,
                requested_paths,
                &candidate.parents[0],
                &parent1_path,
            )?
            else {
                accounting.examine(CandidateOutcome::Irrelevant);
                continue;
            };
            let Some(parent2_entry) = cached_tree_entry(
                runner,
                metadata,
                requested_paths,
                &candidate.parents[1],
                &parent2_path,
            )?
            else {
                accounting.examine(CandidateOutcome::Irrelevant);
                continue;
            };
            // The merge commit may have deleted or renamed the file; a missing
            // result entry keeps the case with the absence disclosed instead
            // of dropping the lead (see #229/#231 intent).
            let result_entry = cached_tree_entry(
                runner,
                metadata,
                requested_paths,
                &candidate.oid,
                &result_path,
            )?;
            let replay = runner.replay(base, &candidate.parents[0], &candidate.parents[1])?;
            if !replay.conflicted {
                accounting.examine(CandidateOutcome::NonConflicting);
                continue;
            }
            if runner.has_unsupported_attributes(&[
                base,
                &candidate.parents[0],
                &candidate.parents[1],
                &candidate.oid,
                &replay.tree,
            ])? {
                skip_candidate(
                    &mut report,
                    accounting,
                    CandidateOutcome::Unsupported,
                    format!(
                        "{} uses an unsupported custom merge attribute; external drivers are disabled",
                        candidate.oid
                    ),
                );
                continue;
            }
            let stage_paths = replay
                .stages
                .iter()
                .map(|stage| stage.path.clone())
                .collect::<BTreeSet<_>>();
            let matching_stages = stage_paths
                .into_iter()
                .filter(|stage_path| {
                    same_incarnation_path(incarnations, &candidate.oid, identity, stage_path)
                })
                .filter_map(|stage_path| {
                    stages_for_path(&replay, &stage_path).map(|stages| (stage_path, stages))
                })
                .collect::<Vec<_>>();
            if matching_stages.is_empty() {
                accounting.examine(CandidateOutcome::Irrelevant);
                continue;
            }
            if matching_stages.len() > 1 {
                skip_candidate(
                    &mut report,
                    accounting,
                    CandidateOutcome::Ambiguous,
                    format!(
                        "{} has multiple conflicts for the current file incarnation",
                        candidate.oid
                    ),
                );
                continue;
            }
            let (stage_path, stages) = matching_stages
                .into_iter()
                .next()
                .expect("one matching stage path");
            let Ok(conflict_path) = String::from_utf8(stage_path) else {
                skip_candidate(
                    &mut report,
                    accounting,
                    CandidateOutcome::Failed,
                    format!("{} conflict path is not UTF-8", candidate.oid),
                );
                continue;
            };
            if !stages.iter().all(|stage| regular_mode(&stage.mode)) {
                skip_candidate(
                    &mut report,
                    accounting,
                    CandidateOutcome::Failed,
                    format!("{} has non-regular conflict stages", candidate.oid),
                );
                continue;
            }
            if !regular_mode(&base_entry.mode)
                || !regular_mode(&parent1_entry.mode)
                || !regular_mode(&parent2_entry.mode)
                || base_entry.oid != stages[0].oid
                || parent1_entry.oid != stages[1].oid
                || parent2_entry.oid != stages[2].oid
            {
                skip_candidate(
                    &mut report,
                    accounting,
                    CandidateOutcome::Failed,
                    format!(
                        "{} tree entries do not match its regular text conflict stages",
                        candidate.oid
                    ),
                );
                continue;
            }
            let Some(conflict_entry) = cached_tree_entry(
                runner,
                metadata,
                requested_paths,
                &replay.tree,
                &conflict_path,
            )?
            else {
                skip_candidate(
                    &mut report,
                    accounting,
                    CandidateOutcome::Failed,
                    format!("{} replay produced no conflict blob", candidate.oid),
                );
                continue;
            };
            if !regular_mode(&conflict_entry.mode) {
                skip_candidate(
                    &mut report,
                    accounting,
                    CandidateOutcome::Failed,
                    format!("{} replay conflict is not a regular file", candidate.oid),
                );
                continue;
            }
            let (Some(base_blob), Some(parent1_blob), Some(parent2_blob), Some(conflict_blob)) = (
                runner.blob(&base_entry.oid)?,
                runner.blob(&parent1_entry.oid)?,
                runner.blob(&parent2_entry.oid)?,
                runner.blob(&conflict_entry.oid)?,
            ) else {
                skip_candidate(
                    &mut report,
                    accounting,
                    CandidateOutcome::Failed,
                    format!("{} contains a non-text or oversized blob", candidate.oid),
                );
                continue;
            };
            let (Some(parent1_text), Some(parent2_text)) = (
                std::str::from_utf8(&parent1_blob).ok(),
                std::str::from_utf8(&parent2_blob).ok(),
            ) else {
                skip_candidate(
                    &mut report,
                    accounting,
                    CandidateOutcome::Failed,
                    format!("{} parent blobs are not valid UTF-8 text", candidate.oid),
                );
                continue;
            };
            let (result, result_status, case_limitation) = match result_entry {
                Some(entry) if regular_mode(&entry.mode) => match runner.blob(&entry.oid)? {
                    Some(blob) => (
                        Some(historical_object(
                            Some(candidate.oid.clone()),
                            &result_path,
                            &entry.oid,
                            &blob,
                        )),
                        "present",
                        String::new(),
                    ),
                    None => (
                        None,
                        "unreadable",
                        format!(
                            "{} committed result is not bounded UTF-8 text; inspect the result blob {} directly",
                            candidate.oid, entry.oid
                        ),
                    ),
                },
                Some(entry) => (
                    Some(HistoricalObject {
                        commit: Some(candidate.oid.clone()),
                        path: result_path.clone(),
                        blob: entry.oid.clone(),
                        excerpt: String::new(),
                        excerpt_truncated: false,
                    }),
                    "non_regular",
                    format!(
                        "{} committed result is not a regular file; inspect the result blob {} directly",
                        candidate.oid, entry.oid
                    ),
                ),
                None => (
                    None,
                    "absent",
                    format!(
                        "{} committed result does not contain the file (deleted or absent); no result excerpt exists",
                        candidate.oid
                    ),
                ),
            };
            let conflict_text =
                std::str::from_utf8(&conflict_blob).expect("blob reader validated UTF-8");
            let Some(historical_regions) = conflict_regions(conflict_text) else {
                skip_candidate(
                    &mut report,
                    accounting,
                    CandidateOutcome::Failed,
                    format!("{} conflict markers could not be parsed", candidate.oid),
                );
                continue;
            };
            let parent1_lines = parent1_text.lines().collect::<Vec<_>>();
            let parent2_lines = parent2_text.lines().collect::<Vec<_>>();
            if !regions_match_parents(&historical_regions, &parent1_lines, &parent2_lines) {
                skip_candidate(
                    &mut report,
                    accounting,
                    CandidateOutcome::Failed,
                    format!(
                        "{} conflict regions could not be validated against the historical parents",
                        candidate.oid
                    ),
                );
                continue;
            }
            let (region, region_strength, association, region_trace, region_limitation) =
                match match_regions(
                    current_text,
                    &current_regions,
                    conflict_text,
                    &historical_regions,
                ) {
                    RegionMatch::Unique {
                        current,
                        historical,
                        basis,
                    } => {
                        let historical_region = &historical_regions[historical];
                        (
                            Some(HistoricalRegion {
                                before: historical_region.before.clone(),
                                after: historical_region.after.clone(),
                                context: "surrounding",
                                result_anchored: false,
                            }),
                            2u8,
                            basis,
                            Some(RegionTrace {
                                current_start_line: current_regions[current].start_line,
                                current_lines: current_regions[current].line_count,
                                historical_start_line: historical_region.start_line,
                                historical_lines: historical_region.line_count,
                            }),
                            String::new(),
                        )
                    }
                    RegionMatch::Ambiguous => (
                        None,
                        1,
                        "same file incarnation; no unique region correspondence",
                        None,
                        format!(
                            "{} conflict context appears more than once in the current or historical conflict; no precise region mapping",
                            candidate.oid
                        ),
                    ),
                    RegionMatch::None => (
                        None,
                        0,
                        "same file incarnation; no region correspondence",
                        None,
                        format!(
                            "{} conflict shares no surrounding context with the current conflict; no region mapping",
                            candidate.oid
                        ),
                    ),
                };
            let mut limitations = Vec::new();
            if !region_limitation.is_empty() {
                limitations.push(region_limitation);
            }
            if !case_limitation.is_empty() {
                limitations.push(case_limitation);
            }
            accounting.examine(CandidateOutcome::Irrelevant);
            staged_cases.push(StagedCase {
                region_strength,
                commit_time: candidate.commit_time,
                case: HistoricalCase {
                    merge_commit: candidate.oid.clone(),
                    merge_base: historical_object(
                        Some(base.clone()),
                        &base_path,
                        &base_entry.oid,
                        &base_blob,
                    ),
                    parents: [
                        historical_object(
                            Some(candidate.parents[0].clone()),
                            &parent1_path,
                            &parent1_entry.oid,
                            &parent1_blob,
                        ),
                        historical_object(
                            Some(candidate.parents[1].clone()),
                            &parent2_path,
                            &parent2_entry.oid,
                            &parent2_blob,
                        ),
                    ],
                    related_sides: candidate.related_sides.iter().copied().collect(),
                    reconstructed_conflict: historical_object(
                        None,
                        &conflict_path,
                        &conflict_entry.oid,
                        &conflict_blob,
                    ),
                    region,
                    result,
                    result_status,
                    limitations,
                    association,
                    region_trace,
                },
            });
        }
        staged_cases.sort_by(|left, right| {
            right
                .region_strength
                .cmp(&left.region_strength)
                .then_with(|| right.commit_time.cmp(&left.commit_time))
                .then_with(|| left.case.merge_commit.cmp(&right.case.merge_commit))
        });
        report.cases_total = staged_cases.len();
        report.cases_truncated = report.cases_total > CASE_LIMIT_PER_FILE;
        report.cases_omitted = report.cases_total.saturating_sub(CASE_LIMIT_PER_FILE);
        if report.cases_truncated {
            report.reasons.push(format!(
                "{} additional ranked cases omitted by the {}-case-per-file limit",
                report.cases_omitted, CASE_LIMIT_PER_FILE
            ));
        }
        report.cases = staged_cases
            .into_iter()
            .take(CASE_LIMIT_PER_FILE)
            .map(|staged| staged.case)
            .collect();
        Ok(report)
    }
}

struct StagedCase {
    region_strength: u8,
    commit_time: i64,
    case: HistoricalCase,
}

fn partial_file(mut file: HistoricalFile, reason: &str) -> Result<HistoricalFile, AppError> {
    file.status = "partial";
    file.reasons.push(reason.to_owned());
    Ok(file)
}

fn skip_candidate(
    file: &mut HistoricalFile,
    accounting: &mut CandidateAccounting,
    outcome: CandidateOutcome,
    reason: String,
) {
    file.status = "partial";
    accounting.examine(outcome);
    file.reasons.push(reason);
}

// Query-local immutable metadata, targeted to the selected incarnations' aliases.
fn cached_tree_entry(
    runner: &MergeTree,
    metadata: &mut HashMap<String, Vec<git::TreeEntry>>,
    requested_paths: &BTreeSet<String>,
    revision: &str,
    path: &str,
) -> Result<Option<git::TreeEntry>, AppError> {
    if path.is_empty() {
        return Ok(None);
    }
    if !metadata.contains_key(revision) {
        metadata.insert(
            revision.to_owned(),
            runner.tree_entries_for_paths(revision, requested_paths)?,
        );
    }
    Ok(metadata[revision]
        .iter()
        .find(|entry| entry.path == path.as_bytes())
        .cloned())
}

fn incarnation_path_at(
    repository: &git::Repository,
    tree_paths: &mut HashMap<String, BTreeSet<Vec<u8>>>,
    incarnations: &FileIncarnationHistory,
    revision: &str,
    identity: &cache::FileIncarnation,
) -> Result<PathMatch, AppError> {
    if !tree_paths.contains_key(revision) {
        tree_paths.insert(revision.to_owned(), repository.file_paths_at(revision)?);
    }
    let available_paths = &tree_paths[revision];
    let paths = incarnations
        .aliases_in(identity, available_paths)
        .into_iter()
        .filter(|path| same_incarnation_path(incarnations, revision, identity, path))
        .filter_map(|path| String::from_utf8(path).ok())
        .collect::<BTreeSet<_>>();
    match paths.len() {
        0 => Ok(PathMatch::Missing),
        1 => Ok(PathMatch::Unique(
            paths.into_iter().next().expect("one path"),
        )),
        _ => Ok(PathMatch::Ambiguous),
    }
}

fn same_incarnation_path(
    incarnations: &FileIncarnationHistory,
    revision: &str,
    identity: &cache::FileIncarnation,
    path: &[u8],
) -> bool {
    if let Some(members) = incarnations.members_at(revision) {
        let has_revision_path = members
            .values()
            .any(|paths| paths.iter().any(|member| member.as_slice() == path));
        if has_revision_path {
            return members
                .get(identity)
                .is_some_and(|paths| paths.iter().any(|member| member.as_slice() == path));
        }
    }

    let path = BTreeSet::from([path.to_vec()]);
    let identities = incarnations.identities_for_paths(&path);
    identities.len() == 1 && identities.contains(identity)
}

fn stages_for_path<'a>(replay: &'a Replay, path: &[u8]) -> Option<[&'a StageEntry; 3]> {
    let stage = |number| {
        replay
            .stages
            .iter()
            .find(|entry| entry.path == path && entry.stage == number)
    };
    Some([stage(1)?, stage(2)?, stage(3)?])
}

fn regular_mode(mode: &[u8]) -> bool {
    matches!(mode, b"100644" | b"100755")
}

fn historical_object(
    commit: Option<String>,
    path: &str,
    blob: &str,
    content: &[u8],
) -> HistoricalObject {
    let text = std::str::from_utf8(content).expect("blob reader validated UTF-8");
    let mut excerpt = text.chars().take(EXCERPT_CHAR_LIMIT).collect::<String>();
    let excerpt_truncated = text.chars().count() > EXCERPT_CHAR_LIMIT;
    if excerpt_truncated {
        excerpt.push_str("\n…");
    }
    HistoricalObject {
        commit,
        path: path.to_owned(),
        blob: blob.to_owned(),
        excerpt,
        excerpt_truncated,
    }
}

fn conflict_regions(text: &str) -> Option<Vec<ConflictRegion>> {
    let lines = text.lines().collect::<Vec<_>>();
    let mut regions = Vec::new();
    let mut cursor = 0;
    while cursor < lines.len() {
        if !lines[cursor].starts_with("<<<<<<<") {
            cursor += 1;
            continue;
        }
        let separator =
            (cursor + 1..lines.len()).find(|index| lines[*index].starts_with("======="))?;
        let end =
            (separator + 1..lines.len()).find(|index| lines[*index].starts_with(">>>>>>>"))?;
        let mut before = Vec::new();
        for line in lines[..cursor].iter().rev() {
            if marker_line(line) || before.len() == 2 {
                break;
            }
            before.push((*line).to_owned());
        }
        before.reverse();
        let mut after = Vec::new();
        for line in &lines[end + 1..] {
            if marker_line(line) || after.len() == 2 {
                break;
            }
            after.push((*line).to_owned());
        }

        let ours_lines = lines[cursor + 1..separator]
            .iter()
            .map(|line| (*line).to_owned())
            .collect();
        let theirs_lines = lines[separator + 1..end]
            .iter()
            .map(|line| (*line).to_owned())
            .collect();
        regions.push(ConflictRegion {
            before,
            after,
            ours_lines,
            theirs_lines,
            start_line: cursor + 1,
            line_count: end - cursor + 1,
        });
        cursor = end + 1;
    }
    (!regions.is_empty()).then_some(regions)
}

fn marker_line(line: &str) -> bool {
    line.starts_with("<<<<<<<")
        || line.starts_with("=======")
        || line.starts_with(">>>>>>>")
        || line.starts_with("|||||||")
}

const CONTEXT_BASIS: &str = "same file incarnation and identical unique surrounding context";
const CONTENT_BASIS: &str = "same file incarnation and unique conflict-region content correspondence across historical edits";

fn match_regions(
    current_text: &str,
    current: &[ConflictRegion],
    historical_text: &str,
    historical: &[ConflictRegion],
) -> RegionMatch {
    let current_lines = current_text.lines().collect::<Vec<_>>();
    let historical_lines = historical_text.lines().collect::<Vec<_>>();
    let mut matches = BTreeSet::new();
    let mut ambiguous = false;
    for (current_index, current_region) in current.iter().enumerate() {
        for (historical_index, historical_region) in historical.iter().enumerate() {
            let mut matched = false;
            for (current_context, historical_context) in [
                (&current_region.before, &historical_region.before),
                (&current_region.after, &historical_region.after),
            ] {
                if current_context.is_empty() || current_context != historical_context {
                    continue;
                }
                if count_context(&current_lines, current_context) == 1
                    && count_context(&historical_lines, historical_context) == 1
                {
                    matches.insert((current_index, historical_index, CONTEXT_BASIS));
                    matched = true;
                } else {
                    ambiguous = true;
                }
            }
            if matched {
                continue;
            }
            if region_content(current_region).is_empty()
                || region_content(historical_region).is_empty()
            {
                continue;
            }
            if region_content(current_region) == region_content(historical_region)
                && count_region_content(&current_lines, current_region) == 1
                && count_region_content(&historical_lines, historical_region) == 1
            {
                matches.insert((current_index, historical_index, CONTENT_BASIS));
            }
        }
    }
    let mut unique: Option<(usize, usize, &'static str)> = None;
    for found in &matches {
        if unique.is_some() {
            return RegionMatch::Ambiguous;
        }
        unique = Some(*found);
    }
    match unique {
        Some((current, historical, basis)) if !ambiguous => RegionMatch::Unique {
            current,
            historical,
            basis,
        },
        Some(_) => RegionMatch::Ambiguous,
        None if ambiguous => RegionMatch::Ambiguous,
        None => RegionMatch::None,
    }
}

fn region_content(region: &ConflictRegion) -> Vec<&str> {
    region
        .ours_lines
        .iter()
        .chain(region.theirs_lines.iter())
        .map(String::as_str)
        .collect()
}
/// Count occurrences of a region's decisive content block: the ours lines,
/// the conflict separator marker, and the theirs lines, contiguous in the
/// conflict text. Used for uniqueness, never as a positional identity test.
fn count_region_content(lines: &[&str], region: &ConflictRegion) -> usize {
    let ours = &region.ours_lines;
    let theirs = &region.theirs_lines;
    let span = ours.len() + theirs.len() + 1;
    if span > lines.len() {
        return 0;
    }
    lines
        .windows(span)
        .filter(|window| {
            let (head, rest) = window.split_at(ours.len());
            let (marker, tail) = rest.split_at(1);
            lines_equal(head, ours) && marker[0].starts_with("=======") && lines_equal(tail, theirs)
        })
        .count()
}

fn lines_equal(window: &[&str], expected: &[String]) -> bool {
    window
        .iter()
        .zip(expected)
        .all(|(line, expected)| *line == expected)
}

/// Verify parsed conflict regions against the historical parent blobs: the
/// ours side of each region must have its non-empty lines present in parent 1
/// and the theirs side in parent 2, so marker scanning alone cannot invent a
/// region whose sides appear in neither parent. (This is per-line containment,
/// not a contiguous-block match.)
fn regions_match_parents(
    regions: &[ConflictRegion],
    parent1_lines: &[&str],
    parent2_lines: &[&str],
) -> bool {
    regions.iter().all(|region| {
        let ours_ok = region
            .ours_lines
            .iter()
            .all(|line| line.is_empty() || parent1_lines.contains(&line.as_str()));
        let theirs_ok = region
            .theirs_lines
            .iter()
            .all(|line| line.is_empty() || parent2_lines.contains(&line.as_str()));
        ours_ok && theirs_ok
    })
}

fn count_context(lines: &[&str], context: &[String]) -> usize {
    if context.len() > lines.len() {
        return 0;
    }
    lines
        .windows(context.len())
        .filter(|window| {
            window
                .iter()
                .zip(context)
                .all(|(line, expected)| *line == expected)
        })
        .count()
}
