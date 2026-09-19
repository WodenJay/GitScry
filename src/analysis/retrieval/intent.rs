use crate::app::AppError;

use super::text::{normalize_path, tokenize};

/// What one natural-language query asks the cache for.
pub(crate) struct Intent {
    terms: Vec<String>,
    /// Repository-relative anchors, from `--path` and from path-like query words.
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
            // Validate before normalizing, because normalizing trims the leading slash that
            // marks an absolute path.
            if anchor.trim().is_empty() {
                return Err(AppError::input("path must not be empty"));
            }
            if !is_repository_relative(&anchor) {
                return Err(AppError::input(format!(
                    "path must be repository-relative: {anchor}"
                )));
            }
            let anchor = normalize_path(anchor.as_bytes());
            if !anchors.contains(&anchor) {
                anchors.push(anchor);
            }
        }
        Ok(Self {
            terms: tokenize(&input),
            anchors,
        })
    }

    pub(super) fn terms(&self) -> &[String] {
        &self.terms
    }

    /// Every anchor the caller supplied, path-like query words included.
    pub(super) fn anchors(&self) -> &[String] {
        &self.anchors
    }
}

/// Whether a caller-supplied path is relative to the repository root.
///
/// An absolute path, a Windows drive, or a traversal escapes the repository, so it is
/// invalid input rather than a query that silently broadens.
fn is_repository_relative(path: &str) -> bool {
    let path = std::path::Path::new(path);
    // `has_root` also catches `/etc/passwd`, which is not `is_absolute` on Windows.
    !path.is_absolute()
        && !path.has_root()
        && path
            .components()
            .all(|component| !matches!(component, std::path::Component::ParentDir))
}

/// Path-like words inside the query text, so `examples provider/tavily.rs` still anchors.
fn path_like_words(input: &str) -> Vec<String> {
    input
        .split_whitespace()
        .filter(|part| part.contains('/') || part.contains('\\') || part.contains('.'))
        .map(str::to_owned)
        .collect()
}
