//! Splitting commit text and paths the way retrieval and display both need.

pub(crate) fn message_parts(message: &[u8]) -> (String, String) {
    let message = String::from_utf8_lossy(message);
    let mut lines = message.splitn(2, '\n');
    let subject = lines
        .next()
        .unwrap_or_default()
        .trim_end_matches('\r')
        .to_owned();
    let body = lines.next().unwrap_or_default().to_owned();
    (subject, body)
}

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
            push_term(&mut terms, &mut current);
        } else {
            if let Some(previous) = previous
                && !current.is_empty()
                && identifier_boundary(previous_previous, previous, character, next)
            {
                push_term(&mut terms, &mut current);
            }
            current.extend(character.to_lowercase());
        }
    }
    push_term(&mut terms, &mut current);
    terms
}

fn push_term(terms: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        if terms.iter().any(|term| term == current) {
            current.clear();
        } else {
            terms.push(std::mem::take(current));
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
