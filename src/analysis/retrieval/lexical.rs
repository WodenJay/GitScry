use std::collections::HashSet;

use super::intent::Intent;
use super::text::{exact_term_matches, normalize_path, path_matches, term_matches, tokenize};

const SUBJECT_WEIGHT: f64 = 1.5;
const COVERAGE_WEIGHT: f64 = 2.0;
const REPOSITORY_WEIGHT: f64 = 1.0;
const ANCHOR_BONUS: f64 = 3.0;

/// How well one commit answers the intent's words: the shared lexical pipeline.
///
/// Two coverage readings are kept because the capabilities ask different questions.
/// `search` retrieves a commit, so only the prose it wrote counts. `examples` and
/// `failures` describe a change, so a query term satisfied by the paths the change touched
/// also counts.
pub(in crate::analysis) struct Signals {
    subject_matches: usize,
    repository_matches: usize,
    exact_subject_matches: usize,
    exact_repository_matches: usize,
    /// Query terms matched by prose, including partial CJK fragments.
    covered: usize,
    partial_coverage: usize,
    /// Query terms matched by prose or paths, including partial CJK fragments.
    identified: usize,
    partial_identified: usize,
    total_terms: usize,
    matched_anchors: usize,
    lexical: f64,
}

pub(in crate::analysis) fn signals(
    intent: &Intent,
    subject: &str,
    body: &str,
    paths: &[Vec<u8>],
    bm25: f64,
) -> Signals {
    let subject_terms = tokenize(subject).into_iter().collect::<HashSet<_>>();
    let body_terms = tokenize(body).into_iter().collect::<HashSet<_>>();
    let path_terms = path_terms(paths);
    let prose = subject_terms
        .union(&body_terms)
        .cloned()
        .collect::<HashSet<_>>();
    let everything = prose.union(&path_terms).cloned().collect::<HashSet<_>>();
    let path_texts = paths
        .iter()
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect::<Vec<_>>();
    let path_text_refs = path_texts.iter().map(String::as_str).collect::<Vec<_>>();
    let mut all_text_refs = vec![subject, body];
    all_text_refs.extend(path_text_refs.iter().copied());

    let subject_matches = count_matches(intent.terms(), &subject_terms, &[subject]);
    let repository_matches = count_matches(intent.terms(), &path_terms, &path_text_refs);
    let prose_matches = count_matches(intent.terms(), &prose, &[subject, body]);
    let identified_matches = count_matches(intent.terms(), &everything, &all_text_refs);
    Signals {
        subject_matches: subject_matches.total(),
        exact_subject_matches: subject_matches.exact,
        repository_matches: repository_matches.total(),
        exact_repository_matches: repository_matches.exact,
        covered: prose_matches.total(),
        partial_coverage: prose_matches.partial,
        identified: identified_matches.total(),
        partial_identified: identified_matches.partial,
        total_terms: intent.terms().len(),
        matched_anchors: anchors_overlap(paths, intent.anchors()),
        lexical: if bm25.is_finite() { -bm25 } else { 0.0 },
    }
}

/// Terms a path contributes, with separators opened so `src/db/index.rs` yields `db`.
fn path_terms(paths: &[Vec<u8>]) -> HashSet<String> {
    paths
        .iter()
        .flat_map(|path| {
            let path = String::from_utf8_lossy(path).replace(['/', '\\', '.', '-', '_'], " ");
            tokenize(&path)
        })
        .chain(
            paths
                .iter()
                .flat_map(|path| tokenize(&String::from_utf8_lossy(path))),
        )
        .collect()
}

#[derive(Clone, Copy, Default)]
struct TermMatches {
    exact: usize,
    partial: usize,
}

impl TermMatches {
    fn total(self) -> usize {
        self.exact + self.partial
    }
}

fn count_matches(
    terms: &[String],
    haystack: &HashSet<String>,
    original_texts: &[&str],
) -> TermMatches {
    let mut matches = TermMatches::default();
    for term in terms {
        if exact_term_matches(term, haystack, original_texts) {
            matches.exact += 1;
        } else if term_matches(term, haystack) {
            matches.partial += 1;
        }
    }
    matches
}

impl Signals {
    /// How many of the intent's terms this commit shares anywhere.
    pub(in crate::analysis) fn shared_terms(&self) -> usize {
        self.identified
    }

    pub(in crate::analysis) fn exact_anchor(&self) -> bool {
        self.matched_anchors > 0
    }

    /// How many caller anchors this commit's paths matched.
    pub(in crate::analysis) fn matched_anchors(&self) -> usize {
        self.matched_anchors
    }

    fn weighted(&self, coverage: usize) -> f64 {
        self.lexical
            + self.subject_matches as f64 * SUBJECT_WEIGHT
            + coverage as f64 / self.total_terms as f64 * COVERAGE_WEIGHT
            + self.repository_matches as f64 * REPOSITORY_WEIGHT
            + if self.exact_anchor() {
                ANCHOR_BONUS
            } else {
                0.0
            }
    }

    /// The score `search` uses: prose the commit wrote.
    pub(in crate::analysis) fn score(&self) -> f64 {
        self.weighted(self.covered)
    }

    /// The score a capability uses when the paths a change touched describe it.
    pub(in crate::analysis) fn identified_score(&self) -> f64 {
        self.weighted(self.identified)
    }

    pub(in crate::analysis) fn strong(&self) -> bool {
        self.exact_subject_matches > 0 && (self.exact_repository_matches > 0 || self.exact_anchor())
    }

    pub(in crate::analysis) fn moderate(&self) -> bool {
        self.exact_subject_matches > 0
            || self.exact_repository_matches > 0
            || self.covered > 1
            || self.identified > 1
            || self.exact_anchor()
    }

    pub(in crate::analysis) fn describe(&self, basis: &mut Vec<String>) {
        self.describe_coverage(self.covered, self.partial_coverage, basis);
    }

    /// Describe the coverage a capability actually scored on.
    pub(in crate::analysis) fn describe_identified(&self, basis: &mut Vec<String>) {
        self.describe_coverage(self.identified, self.partial_identified, basis);
    }

    fn describe_coverage(&self, coverage: usize, partial: usize, basis: &mut Vec<String>) {
        if self.exact_subject_matches > 0 {
            basis.push(format!("subject match ({})", self.exact_subject_matches));
        }
        basis.push(format!("term coverage {coverage}/{}", self.total_terms));
        if partial > 0 {
            basis.push(format!("partial CJK fragment match ({partial})"));
        }
        if self.exact_repository_matches > 0 {
            basis.push(format!(
                "exact repository term ({})",
                self.exact_repository_matches
            ));
        }
        if self.exact_anchor() {
            basis.push("exact path match".to_owned());
        }
    }
}

/// How many anchors point at a path this commit changed.
pub(in crate::analysis) fn anchors_overlap(paths: &[Vec<u8>], anchors: &[String]) -> usize {
    anchors
        .iter()
        .filter(|anchor| {
            let anchor = normalize_path(anchor.as_bytes());
            !anchor.is_empty() && paths.iter().any(|path| path_matches(&anchor, path))
        })
        .count()
}
