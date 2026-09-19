use crate::app::AppError;

use super::text::{normalize_path, tokenize};

/// What one natural-language query asks the cache for.
pub(crate) struct Intent {
    terms: Vec<String>,
    anchors: Vec<String>,
}

impl Intent {
    pub(crate) fn parse(words: &[String], paths: &[String]) -> Result<Self, AppError> {
        let input = words.join(" ");
        if input.trim().is_empty() {
            return Err(AppError::input("query must not be empty"));
        }
        let mut anchors = Vec::new();
        for anchor in path_like_words(&input)
            .into_iter()
            .chain(paths.iter().cloned())
        {
            let anchor = normalize_path(anchor.as_bytes());
            if anchor.is_empty() {
                return Err(AppError::input("path must not be empty"));
            }
            if !anchors.contains(&anchor) {
                anchors.push(anchor);
            }
        }
        Ok(Self {
            terms: tokenize(&input),
            anchors,
        })
    }

    pub(crate) fn terms(&self) -> &[String] {
        &self.terms
    }

    /// Every anchor the caller supplied, path-like query words included.
    pub(crate) fn anchors(&self) -> &[String] {
        &self.anchors
    }
}

/// Path-like words inside the query text, so `examples provider/tavily.rs` still anchors.
fn path_like_words(input: &str) -> Vec<String> {
    input
        .split_whitespace()
        .filter(|part| part.contains('/') || part.contains('\\') || part.contains('.'))
        .map(str::to_owned)
        .collect()
}
