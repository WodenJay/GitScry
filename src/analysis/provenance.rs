//! Reading failure provenance out of commit text.
//!
//! One reason to change: how GitScry reads a revert marker, a stated failure reason, or a
//! recorded safe retry condition out of what a commit message actually says. Everything
//! here is stated text from history — nothing is inferred, and absence stays absent.

/// Bounds on a quoted reason, so material stays concise.
const MAX_REASON: usize = 240;
const MIN_REASON: usize = 24;

/// Markers that a sentence states why a change failed or was abandoned.
const RATIONALE_MARKERS: &[&str] = &[
    "because",
    "since ",
    "due to",
    "reason:",
    "why:",
    "root cause",
    "caused by",
    "could not",
    "cannot ",
    "can't ",
    "fails",
    "failed",
    "breaks",
    "broke",
    "bricking",
    "regression",
    "no longer",
    "silently",
];

/// Markers that a message records how to retry the abandoned approach safely.
const RETRY_MARKERS: &[&str] = &[
    "re-land criteria:",
    "reland criteria:",
    "re-land:",
    "retry condition:",
    "safe retry:",
    "retry:",
    "workaround:",
    "should be re-addressed",
    "to retry",
];

/// Whether a subject reads like a revert or rollback of earlier work.
pub(super) fn is_revert_subject(subject: &str) -> bool {
    starts_with_any(subject, &["revert", "rollback", "roll back", "back out"])
}

/// Whether a subject reads like a correction of earlier work.
pub(super) fn is_corrective_subject(subject: &str) -> bool {
    starts_with_any(
        subject,
        &[
            "revert",
            "rollback",
            "roll back",
            "back out",
            "fix",
            "correct",
            "restore",
            "follow-up",
            "followup",
        ],
    )
}

fn starts_with_any(subject: &str, prefixes: &[&str]) -> bool {
    let lowered = subject.trim_start().to_ascii_lowercase();
    prefixes.iter().any(|prefix| lowered.starts_with(prefix))
}

/// The object ID a `This reverts commit <oid>` trailer names, if the message has one.
pub(super) fn reverted_commit(text: &str) -> Option<String> {
    let lowered = text.to_ascii_lowercase();
    let marker = "reverts commit ";
    let mut cursor = 0;
    while let Some(found) = lowered[cursor..].find(marker) {
        let start = cursor + found + marker.len();
        let hex = text[start..]
            .chars()
            .take_while(char::is_ascii_hexdigit)
            .collect::<String>();
        if hex.len() >= 7 {
            return Some(hex);
        }
        cursor = start;
    }
    None
}

/// A failure reason, but only one history states.
pub(super) fn stated_reason(subject: &str, body: &str) -> Option<String> {
    for sentence in sentences(body) {
        if contains_marker(&sentence, RATIONALE_MARKERS) {
            let reason = clean(&sentence);
            if reason.len() >= MIN_REASON {
                return Some(reason);
            }
        }
    }
    let subject = strip_conventional_prefix(subject);
    contains_marker(subject, RATIONALE_MARKERS)
        .then(|| clean(subject))
        .filter(|reason| reason.len() >= MIN_REASON)
}

/// The rationale a revert message records.
///
/// A revert body is where the reason is written down, so the first substantive sentence
/// counts even without a rationale marker. Boilerplate trailer lines are not a reason.
pub(super) fn revert_reason(subject: &str, body: &str) -> Option<String> {
    if let Some(reason) = stated_reason(subject, body) {
        return Some(reason);
    }
    for sentence in sentences(body) {
        let reason = clean(&sentence);
        if reason.len() >= MIN_REASON && !is_boilerplate(&reason) {
            return Some(reason);
        }
    }
    None
}

/// Whether a line is revert bookkeeping rather than a stated reason.
fn is_boilerplate(sentence: &str) -> bool {
    let lowered = sentence.to_ascii_lowercase();
    [
        "this reverts commit",
        "reverts commit ",
        "co-authored-by",
        "signed-off-by",
        "reviewed-by",
        "refs ",
        "see ",
    ]
    .iter()
    .any(|marker| lowered.starts_with(marker))
}

/// A safe retry condition, but only one history records.
pub(super) fn stated_retry(text: &str) -> Option<String> {
    for sentence in sentences(text) {
        let lowered = sentence.to_ascii_lowercase();
        let Some((index, marker)) = RETRY_MARKERS
            .iter()
            .filter_map(|marker| lowered.find(marker).map(|index| (index, *marker)))
            .min_by_key(|(index, _)| *index)
        else {
            continue;
        };
        let retry = clean(&sentence[index + marker.len()..]);
        let retry = retry
            .strip_prefix("a ")
            .or_else(|| retry.strip_prefix("the "))
            .unwrap_or(&retry);
        if retry.len() >= MIN_REASON {
            return Some(retry.to_owned());
        }
    }
    None
}

fn contains_marker(sentence: &str, markers: &[&str]) -> bool {
    let lowered = sentence.to_ascii_lowercase();
    markers.iter().any(|marker| lowered.contains(marker))
}

/// Sentence-bounded windows, so a reason never spans unrelated paragraphs.
fn sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            if !current.trim().is_empty() {
                sentences.push(std::mem::take(&mut current));
            }
            continue;
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(line);
        for sentence in split_sentences(&std::mem::take(&mut current)) {
            sentences.push(sentence);
        }
    }
    if !current.trim().is_empty() {
        sentences.push(current);
    }
    sentences
}

fn split_sentences(line: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    for piece in line.split_inclusive(['.', '!', '?']) {
        current.push_str(piece);
        if piece.ends_with(['.', '!', '?']) {
            sentences.push(std::mem::take(&mut current));
        }
    }
    if !current.trim().is_empty() {
        sentences.push(current);
    }
    sentences
}

/// Drop a `type(scope):` prefix so a subject reads as prose.
fn strip_conventional_prefix(subject: &str) -> &str {
    let subject = subject.trim();
    let Some((head, tail)) = subject.split_once(':') else {
        return subject;
    };
    if head.is_empty() || head.len() > 24 || head.contains(' ') {
        return subject;
    }
    tail.trim()
}

fn clean(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    for character in text.trim().chars() {
        if character.is_control() {
            cleaned.push(' ');
        } else {
            cleaned.push(character);
        }
    }
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned = cleaned.trim_matches(['-', ':', '"', ' ', '.']).to_owned();
    if cleaned.chars().count() > MAX_REASON {
        let mut bounded = cleaned.chars().take(MAX_REASON - 1).collect::<String>();
        bounded.push('…');
        bounded
    } else {
        cleaned
    }
}
