//! Normalizing text and paths the way retrieval and display both need.

use std::collections::HashSet;

/// Prose plus the identifier terms it contains, so `MATCH` can reach inside identifiers.
pub(crate) fn searchable_text(value: &str) -> String {
    let terms = tokenize(value);
    if terms.is_empty() {
        value.to_owned()
    } else {
        format!("{value}\n{}", terms.join(" "))
    }
}

pub(in crate::analysis) fn tokenize(input: &str) -> Vec<String> {
    let logical_terms = logical_terms(input);
    let mut terms = Vec::new();
    let mut seen = HashSet::new();
    for term in logical_terms {
        push_unique(&mut terms, &mut seen, term.clone());
        if is_cjk_run(&term) {
            push_cjk_fragments(&term, true, &mut terms, &mut seen);
        }
    }
    terms
}

/// Logical query terms and FTS alternatives; longer CJK terms omit single-character fallbacks.
pub(super) fn query_terms(input: &str) -> (Vec<String>, Vec<String>) {
    let terms = logical_terms(input);
    let mut matching_terms = Vec::new();
    let mut seen = HashSet::new();
    for term in &terms {
        push_unique(&mut matching_terms, &mut seen, term.clone());
        if is_cjk_run(term) && term.chars().count() > 1 {
            push_cjk_fragments(term, false, &mut matching_terms, &mut seen);
        }
    }
    (terms, matching_terms)
}

/// Match a logical term exactly in original CJK text or in the existing token vocabulary.
pub(super) fn exact_term_matches(
    term: &str,
    haystack: &HashSet<String>,
    original_texts: &[&str],
) -> bool {
    if is_cjk_run(term) {
        original_texts.iter().any(|text| text.contains(term))
    } else {
        haystack.contains(term)
    }
}

/// Match one logical term against its indexed whole token or CJK bigrams.
fn cjk_bigrams(term: &str) -> impl Iterator<Item = (char, char)> + '_ {
    term.chars().zip(term.chars().skip(1))
}
pub(super) fn term_matches(term: &str, haystack: &HashSet<String>) -> bool {
    if haystack.contains(term) {
        return true;
    }
    if !is_cjk_run(term) {
        return false;
    }
    cjk_bigrams(term).any(|(previous, current)| {
        let fragment = [previous, current].iter().collect::<String>();
        haystack.contains(&fragment)
    })
}

fn logical_terms(input: &str) -> Vec<String> {
    let chars = input.chars().collect::<Vec<_>>();
    let mut terms = Vec::new();
    let mut seen = HashSet::new();
    let mut current = String::new();
    let mut current_is_cjk = None;
    for (index, character) in chars.iter().copied().enumerate() {
        let next = chars.get(index + 1).copied();
        let previous = index
            .checked_sub(1)
            .and_then(|index| chars.get(index).copied());
        let previous_previous = index
            .checked_sub(2)
            .and_then(|index| chars.get(index).copied());
        if !character.is_alphanumeric() {
            push_term(&mut terms, &mut seen, &mut current);
            current_is_cjk = None;
        } else {
            let character_is_cjk = is_cjk(character);
            if current_is_cjk.is_some_and(|current_is_cjk| current_is_cjk != character_is_cjk) {
                push_term(&mut terms, &mut seen, &mut current);
            } else if !character_is_cjk
                && let Some(previous) = previous
                && !current.is_empty()
                && identifier_boundary(previous_previous, previous, character, next)
            {
                push_term(&mut terms, &mut seen, &mut current);
            }
            current.extend(character.to_lowercase());
            current_is_cjk = Some(character_is_cjk);
        }
    }
    push_term(&mut terms, &mut seen, &mut current);
    terms
}

fn push_cjk_fragments(
    term: &str,
    include_single_characters: bool,
    terms: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    if include_single_characters && let Some(first) = term.chars().next() {
        push_unique(terms, seen, first.to_string());
    }
    for (previous, current) in cjk_bigrams(term) {
        if include_single_characters {
            push_unique(terms, seen, current.to_string());
        }
        push_unique(terms, seen, [previous, current].iter().collect());
    }
}

fn push_unique(terms: &mut Vec<String>, seen: &mut HashSet<String>, term: String) {
    if seen.insert(term.clone()) {
        terms.push(term);
    }
}

fn is_cjk_run(term: &str) -> bool {
    !term.is_empty() && term.chars().all(is_cjk)
}

fn is_cjk(character: char) -> bool {
    matches!(
        character as u32,
        0x1100..=0x11ff
            | 0x3040..=0x30ff
            | 0x3130..=0x318f
            | 0x31f0..=0x31ff
            | 0x3400..=0x4dbf
            | 0x4e00..=0x9fff
            | 0xac00..=0xd7af
            | 0xf900..=0xfaff
            | 0xff66..=0xff9d
            | 0x20000..=0x323af
    )
}

fn push_term(terms: &mut Vec<String>, seen: &mut HashSet<String>, current: &mut String) {
    if !current.is_empty() {
        if seen.insert(current.clone()) {
            terms.push(std::mem::take(current));
        } else {
            current.clear();
        }
    }
}

fn identifier_boundary(
    previous_previous: Option<char>,
    previous: char,
    current: char,
    next: Option<char>,
) -> bool {
    (previous.is_lowercase() && current.is_uppercase())
        || (previous.is_alphabetic() && current.is_numeric())
        || (previous.is_numeric() && current.is_alphabetic())
        || (previous_previous.is_some_and(char::is_uppercase)
            && previous.is_uppercase()
            && current.is_uppercase()
            && next.is_some_and(char::is_lowercase))
}

pub(crate) fn normalize_path(path: &[u8]) -> String {
    let path = String::from_utf8_lossy(path);
    let mut path = path.replace('\\', "/").to_ascii_lowercase();
    while let Some(stripped) = path.strip_prefix("./") {
        path = stripped.to_owned();
    }
    path.trim_matches('/').to_owned()
}

/// Whether a repository-relative query path points at a changed path.
pub(super) fn path_matches(query_path: &str, path: &[u8]) -> bool {
    let path = normalize_path(path);
    path == query_path || (!query_path.contains('/') && path.rsplit('/').next() == Some(query_path))
}

#[cfg(test)]
mod tests {
    use super::{HashSet, tokenize};

    #[test]
    fn repeated_tokens_keep_first_occurrence_order() {
        assert_eq!(
            tokenize("alpha beta alpha gamma beta"),
            ["alpha", "beta", "gamma"],
        );
    }

    #[test]
    fn identifiers_split_and_tokens_normalize_to_lowercase() {
        assert_eq!(
            tokenize("parseHTTP2Response PARSE_http_response"),
            ["parse", "http", "2", "response"],
        );
    }

    #[test]
    fn empty_input_has_no_tokens() {
        assert!(tokenize("").is_empty());
    }

    #[test]
    fn cjk_fragments_overlap_and_deduplicate_deterministically() {
        let terms = tokenize("缓存更新缓存");
        assert_eq!(terms, tokenize("缓存更新缓存 缓存更新缓存"));
        let unique = terms.iter().collect::<HashSet<_>>();
        assert_eq!(unique.len(), terms.len());
        for fragment in ["缓", "存", "更", "新", "缓存", "存更", "更新"] {
            assert!(unique.contains(&fragment.to_owned()), "missing {fragment}");
        }
        assert!(!tokenize("缓存，更新").contains(&"存更".to_owned()));
    }

    #[test]
    fn large_repeated_cjk_input_deduplicates_overlapping_fragments() {
        let input = "缓存".repeat(20_000);
        let terms = tokenize(&input);
        assert!(terms.len() <= 5);
        assert_eq!(terms.iter().collect::<HashSet<_>>().len(), terms.len());
    }

    #[test]
    fn large_repeated_token_input_deduplicates_once() {
        let input = "repeated ".repeat(20_000);
        assert_eq!(tokenize(&input), ["repeated"]);
    }

    #[test]
    fn large_unique_token_input_remains_linear() {
        let input = (0..20_000)
            .map(|index| format!("term{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let tokens = tokenize(&input);
        assert_eq!(tokens.len(), 20_001);
        assert_eq!(tokens.first().map(String::as_str), Some("term"));
        assert_eq!(tokens.get(1).map(String::as_str), Some("0"));
        assert_eq!(tokens.last().map(String::as_str), Some("19999"));
    }
}
