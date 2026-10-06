//! Reading failure provenance out of commit text.
//!
//! One reason to change: how GitScry reads a revert marker, a stated failure reason, or a
//! recorded safe retry condition out of what a commit message actually says. Everything
//! here is stated text from history — nothing is inferred, and absence stays absent.

/// Bounds on a quoted reason, so material stays concise.
const MAX_REASON: usize = 240;
const MIN_REASON: usize = 24;

/// Markers of a sentence that states why a change failed: a cause, or the harm it did.
const CAUSE_MARKERS: &[&str] = &[
    "because",
    "since ",
    "due to",
    "reason:",
    "cannot ",
    "can't ",
    "reverted ",
    "reverts the changes from ",
    "this is a revert of",
    "why:",
    "root cause",
    "caused by",
    "so that",
    "therefore",
    "thus ",
    "could not",
    "cannot ",
    "can't ",
];

/// Markers of a sentence that reports harm an approach did.
const HARM_MARKERS: &[&str] = &[
    "fails",
    "failed",
    "breaks",
    "broke",
    "bricking",
    "bricked",
    "regression",
    "no longer",
    "silently",
    "slowed",
    "slows",
    "clobber",
    "corrupt",
    "leak",
    "race",
    "overhead",
    "stall",
    "hangs",
    "flaky",
    "unstable",
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
            "re-land",
            "reland",
            "re-landed",
            "un-revert",
        ],
    )
}

fn starts_with_any(subject: &str, prefixes: &[&str]) -> bool {
    let lowered = subject.trim_start().to_ascii_lowercase();
    prefixes.iter().any(|prefix| lowered.starts_with(prefix))
}

/// The object IDs explicit revert declarations in this message name, in order.
///
/// Only affirmative, already-performed declarations count, and only the wording history
/// uses to name the undone work directly:
///
/// - `This reverts commit <oid>`
/// - `This is a revert of commit <oid>` or `This is a revert of the code changes in commit <oid>`
/// - `Reverts the changes from commit <oid>`
/// - `Reverted <oid> because <explanation>`
///
/// Declarations are matched case-insensitively across wrapped lines, must start a sentence,
/// and never match plans, negations, or quotations of someone else's message.
pub(in crate::analysis) fn explicit_revert_declarations(text: &str) -> Vec<String> {
    let mut declarations = Vec::new();
    for sentence in sentences(text) {
        let lowered = sentence.to_ascii_lowercase();
        for marker in DECLARATION_MARKERS {
            let mut cursor = 0;
            while let Some(found) = lowered[cursor..].find(marker) {
                let start = cursor + found + marker.len();
                let hex = lowered[start..]
                    .chars()
                    .take_while(char::is_ascii_hexdigit)
                    .collect::<String>();
                if hex.len() >= 7 {
                    declarations.push(hex);
                }
                cursor = start;
            }
        }
    }
    declarations
}

/// Sentence starts that name undone work with an object ID directly.
///
/// A declaration must open the sentence, so a negated or planned revert ("does not revert",
/// "will revert") and a quoted example never match: they cannot precede the marker inside
/// the matched window.
const DECLARATION_MARKERS: &[&str] = &[
    "this reverts commit ",
    "this is a revert of commit ",
    "this is a revert of the code changes in commit ",
    "reverts the changes from commit ",
    "reverted ",
];

/// A failure reason, but only one history states.
pub(super) fn stated_reason(subject: &str, body: &str) -> Option<String> {
    // The reason is stated wherever the message states it, so the first labelled sentence
    // wins rather than a preferred kind of label.
    for sentence in sentences(body) {
        if contains_marker(&sentence, CAUSE_MARKERS) || contains_marker(&sentence, HARM_MARKERS) {
            let reason = clean(&sentence);
            if reason.len() >= MIN_REASON {
                return Some(reason);
            }
        }
    }
    let subject = strip_conventional_prefix(subject);
    (contains_marker(subject, CAUSE_MARKERS) || contains_marker(subject, HARM_MARKERS))
        .then(|| clean(subject))
        .filter(|reason| reason.len() >= MIN_REASON)
}

/// The rationale a revert message records.
///
/// A revert body is where a reason is written down, so a sentence that states harm or cause
/// counts even without an explicit `reason:` label. A revert that records only bookkeeping
/// states no reason, and then `Reason unknown` is the honest answer.
pub(super) fn revert_reason(subject: &str, body: &str) -> Option<String> {
    if let Some(reason) = stated_reason(subject, body) {
        return Some(reason);
    }
    for sentence in sentences(body) {
        let reason = clean(&sentence);
        if reason.len() >= MIN_REASON
            && !is_boilerplate(&reason)
            && (contains_marker(&reason, CAUSE_MARKERS) || contains_marker(&reason, HARM_MARKERS))
        {
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
///
/// Wrapped commit bodies break sentences across lines, so an unfinished sentence is carried
/// to the next line rather than being emitted as a complete one.
fn sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            // A blank line ends the paragraph, and so any sentence still open in it.
            if !current.trim().is_empty() {
                sentences.push(std::mem::take(&mut current));
            }
            continue;
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(line);
        let (complete, remainder) = split_sentences(&current);
        sentences.extend(complete);
        current = remainder;
    }
    if !current.trim().is_empty() {
        sentences.push(current);
    }
    sentences
}

/// Split on sentence enders, returning the finished sentences and the open remainder.
///
/// An ender only closes a sentence at a word boundary, so a period inside
/// `@executable_path/../lib` or `2.0` does not break the sentence it sits in.
fn split_sentences(line: &str) -> (Vec<String>, String) {
    let characters = line.chars().collect::<Vec<_>>();
    let mut sentences = Vec::new();
    let mut current = String::new();
    for (index, character) in characters.iter().copied().enumerate() {
        current.push(character);
        if !matches!(character, '.' | '!' | '?') {
            continue;
        }
        let next = characters.get(index + 1).copied();
        if next.is_none_or(|next| next.is_whitespace()) {
            sentences.push(std::mem::take(&mut current));
        }
    }
    (sentences, current)
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
