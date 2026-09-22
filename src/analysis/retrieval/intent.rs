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
        let anchors = normalize_anchors(
            path_like_words(&input)
                .into_iter()
                .chain(paths.iter().cloned()),
        )?;
        Ok(Self {
            terms: tokenize(&input),
            anchors,
        })
    }

    pub(crate) fn symptom(words: &[String], path: &str) -> Result<Self, AppError> {
        let input = words.join(" ");
        if input.trim().is_empty() {
            return Err(AppError::input("query must not be empty"));
        }
        Ok(Self {
            terms: tokenize(&input),
            anchors: normalize_anchors([path.to_owned()])?,
        })
    }

    pub(crate) fn paths(paths: &[String]) -> Result<Self, AppError> {
        if paths.is_empty() {
            return Err(AppError::input("at least one path is required"));
        }
        Ok(Self {
            terms: Vec::new(),
            anchors: normalize_anchors(paths.iter().cloned())?,
        })
    }

    pub(in crate::analysis) fn terms(&self) -> &[String] {
        &self.terms
    }

    /// Every anchor the caller supplied, path-like query words included.
    pub(crate) fn anchors(&self) -> &[String] {
        &self.anchors
    }
}

fn normalize_anchors(anchors: impl IntoIterator<Item = String>) -> Result<Vec<String>, AppError> {
    let mut normalized = Vec::new();
    for anchor in anchors {
        if anchor.trim().is_empty() {
            return Err(AppError::input("path must not be empty"));
        }
        if !is_repository_relative(&anchor) {
            return Err(AppError::input(format!(
                "path must be repository-relative: {anchor}"
            )));
        }
        let anchor = normalize_path(anchor.as_bytes());
        if anchor.is_empty() {
            return Err(AppError::input("path must not be empty"));
        }
        if !normalized.contains(&anchor) {
            normalized.push(anchor);
        }
    }
    Ok(normalized)
}

/// Whether a caller-supplied path is relative to the repository root.
///
/// An absolute path, a Windows drive, or a traversal escapes the repository, so it is
/// invalid input rather than a query that silently broadens.
fn is_repository_relative(path: &str) -> bool {
    let bytes = path.as_bytes();
    let has_windows_drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    let path = std::path::Path::new(path);
    // `has_root` also catches `/etc/passwd`, which is not `is_absolute` on Windows.
    !path.is_absolute()
        && !path.has_root()
        && !has_windows_drive
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
