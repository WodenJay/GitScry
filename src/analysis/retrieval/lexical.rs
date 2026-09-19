use std::collections::HashSet;

use super::intent::Intent;
use super::text::{normalize_path, path_matches, tokenize};

const SUBJECT_WEIGHT: f64 = 1.5;
const COVERAGE_WEIGHT: f64 = 2.0;
const REPOSITORY_WEIGHT: f64 = 1.0;
const ANCHOR_BONUS: f64 = 3.0;

/// How well one commit answers the intent's words: the shared lexical pipeline.
pub(in crate::analysis) struct Signals {
    subject_matches: usize,
    repository_matches: usize,
    covered: usize,
    total_terms: usize,
    matched_anchors: usize,
    lexical: f64,
}

pub(in crate::analysis) fn signals(
    intent: &Intent,
    subject: &str,
    paths: &[Vec<u8>],
    bm25: f64,
) -> Signals {
    let subject_terms = tokenize(subject).into_iter().collect::<HashSet<_>>();
    let path_terms = paths
        .iter()
        .flat_map(|path| tokenize(&String::from_utf8_lossy(path)))
        .collect::<HashSet<_>>();
    let document_terms = subject_terms
        .union(&path_terms)
        .cloned()
        .collect::<HashSet<_>>();
    Signals {
        subject_matches: count(intent.terms(), &subject_terms),
        repository_matches: count(intent.terms(), &path_terms),
        covered: count(intent.terms(), &document_terms),
        total_terms: intent.terms().len(),
        matched_anchors: anchors_overlap(paths, intent.anchors()),
        lexical: if bm25.is_finite() { -bm25 } else { 0.0 },
    }
}

fn count(terms: &[String], haystack: &HashSet<String>) -> usize {
    terms.iter().filter(|term| haystack.contains(*term)).count()
}

impl Signals {
    /// How many of the intent's terms this commit shares anywhere.
    pub(in crate::analysis) fn shared_terms(&self) -> usize {
        self.covered
    }

    pub(in crate::analysis) fn exact_anchor(&self) -> bool {
        self.matched_anchors > 0
    }

    /// How many caller anchors this commit's paths matched.
    pub(in crate::analysis) fn matched_anchors(&self) -> usize {
        self.matched_anchors
    }

    fn coverage(&self) -> f64 {
        self.covered as f64 / self.total_terms as f64
    }

    pub(in crate::analysis) fn score(&self) -> f64 {
        self.lexical
            + self.subject_matches as f64 * SUBJECT_WEIGHT
            + self.coverage() * COVERAGE_WEIGHT
            + self.repository_matches as f64 * REPOSITORY_WEIGHT
            + if self.exact_anchor() {
                ANCHOR_BONUS
            } else {
                0.0
            }
    }

    pub(in crate::analysis) fn strong(&self) -> bool {
        self.subject_matches > 0 && (self.repository_matches > 0 || self.exact_anchor())
    }

    pub(in crate::analysis) fn moderate(&self) -> bool {
        self.subject_matches > 0
            || self.repository_matches > 0
            || self.covered > 1
            || self.exact_anchor()
    }

    pub(in crate::analysis) fn describe(&self, basis: &mut Vec<String>) {
        if self.subject_matches > 0 {
            basis.push(format!("subject match ({})", self.subject_matches));
        }
        basis.push(format!(
            "term coverage {}/{}",
            self.covered, self.total_terms
        ));
        if self.repository_matches > 0 {
            basis.push(format!(
                "exact repository term ({})",
                self.repository_matches
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
