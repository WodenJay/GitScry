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
    let chars = input.chars().collect::<Vec<_>>();
    let mut terms = Vec::new();
    let mut seen = HashSet::new();
    let mut current = String::new();
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
        } else {
            if let Some(previous) = previous
                && !current.is_empty()
                && identifier_boundary(previous_previous, previous, character, next)
            {
                push_term(&mut terms, &mut seen, &mut current);
            }
            current.extend(character.to_lowercase());
        }
    }
    push_term(&mut terms, &mut seen, &mut current);
    terms
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
    use super::tokenize;

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
