use std::{cmp::Ordering, collections::HashSet};

use rusqlite::{Connection, params};

use crate::app::AppError;

pub(crate) struct SearchReport {
    pub(crate) materials: Vec<Material>,
    pub(crate) matched_count: usize,
    pub(crate) truncated: bool,
}

pub(crate) struct Material {
    pub(crate) oid: String,
    pub(crate) abbreviation: String,
    pub(crate) subject: String,
    pub(crate) paths: Vec<Vec<u8>>,
    pub(crate) confidence: Confidence,
    pub(crate) basis: Vec<String>,
}

pub(crate) enum Confidence {
    High,
    Medium,
    Low,
}

impl Confidence {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

const CANDIDATE_MULTIPLIER: usize = 20;

struct Query {
    terms: Vec<String>,
    paths: Vec<String>,
}

struct Candidate {
    oid: String,
    commit_time: i64,
    subject: String,
    body: String,
    bm25: f64,
    paths: Vec<Vec<u8>>,
}

struct RankedMaterial {
    material: Material,
    score: f64,
    commit_time: i64,
}

pub(crate) fn search(
    connection: &Connection,
    input: &str,
    limit: usize,
) -> Result<SearchReport, AppError> {
    if limit == 0 {
        return Err(AppError::input("limit must be greater than zero"));
    }

    let query = analyze(input);
    if query.terms.is_empty() {
        return Ok(empty_report());
    }

    search_connection(connection, &query, limit)
}

fn search_connection(
    connection: &Connection,
    query: &Query,
    limit: usize,
) -> Result<SearchReport, AppError> {
    let match_query = query
        .terms
        .iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    let matched_count = connection
        .query_row(
            "SELECT COUNT(*) FROM search_fts WHERE search_fts MATCH ?1",
            [&match_query],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| search_error("counting search matches", error))?;
    let matched_count = usize::try_from(matched_count)
        .map_err(|_| search_error("counting search matches", "count exceeded platform limits"))?;
    if matched_count == 0 {
        return Ok(empty_report());
    }

    let candidate_limit =
        i64::try_from(limit.saturating_mul(CANDIDATE_MULTIPLIER)).unwrap_or(i64::MAX);
    let mut candidates = {
        let mut statement = connection
            .prepare(
                "SELECT c.oid, c.commit_time, c.message, bm25(search_fts, 10.0, 3.0, 2.0) FROM search_fts JOIN search_documents AS d ON d.rowid = search_fts.rowid JOIN commits AS c ON c.oid = d.commit_oid WHERE search_fts MATCH ?1 ORDER BY bm25(search_fts, 10.0, 3.0, 2.0), c.commit_time DESC, c.oid ASC LIMIT ?2",
            )
            .map_err(|error| search_error("preparing search", error))?;
        let rows = statement
            .query_map(params![match_query, candidate_limit], |row| {
                let message: Vec<u8> = row.get(2)?;
                let (subject, body) = message_parts(&message);
                Ok(Candidate {
                    oid: row.get(0)?,
                    commit_time: row.get(1)?,
                    subject,
                    body,
                    bm25: row.get(3)?,
                    paths: Vec::new(),
                })
            })
            .map_err(|error| search_error("running search", error))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| search_error("reading search results", error))?
    };

    let mut ranked = Vec::with_capacity(candidates.len());
    for candidate in &mut candidates {
        candidate.paths = changed_paths(connection, &candidate.oid)?;
        ranked.push(rank(candidate, query));
    }
    ranked.sort_by(compare_ranked);

    let mut materials = ranked
        .into_iter()
        .take(limit)
        .map(|ranked| ranked.material)
        .collect::<Vec<_>>();
    assign_abbreviations(&mut materials);

    Ok(SearchReport {
        materials,
        matched_count,
        truncated: matched_count > limit,
    })
}

fn rank(candidate: &Candidate, query: &Query) -> RankedMaterial {
    let subject_terms = tokenize(&candidate.subject)
        .into_iter()
        .collect::<HashSet<_>>();
    let body_terms = tokenize(&candidate.body)
        .into_iter()
        .collect::<HashSet<_>>();
    let path_terms = candidate
        .paths
        .iter()
        .flat_map(|path| tokenize(&String::from_utf8_lossy(path)))
        .collect::<HashSet<_>>();
    let document_terms = subject_terms
        .union(&body_terms)
        .cloned()
        .collect::<HashSet<_>>()
        .union(&path_terms)
        .cloned()
        .collect::<HashSet<_>>();
    let covered = query
        .terms
        .iter()
        .filter(|term| document_terms.contains(*term))
        .count();
    let subject_matches = query
        .terms
        .iter()
        .filter(|term| subject_terms.contains(*term))
        .count();
    let repository_matches = query
        .terms
        .iter()
        .filter(|term| path_terms.contains(*term))
        .count();
    let exact_path = query.paths.iter().any(|query_path| {
        candidate
            .paths
            .iter()
            .any(|path| path_matches(query_path, path))
    });
    let coverage = covered as f64 / query.terms.len() as f64;
    let lexical = if candidate.bm25.is_finite() {
        -candidate.bm25
    } else {
        0.0
    };
    let score = lexical
        + subject_matches as f64 * 1.5
        + coverage * 2.0
        + repository_matches as f64 * 1.0
        + if exact_path { 3.0 } else { 0.0 };

    let mut basis = Vec::new();
    if subject_matches > 0 {
        basis.push(format!("subject match ({subject_matches})"));
    }
    basis.push(format!("term coverage {covered}/{}", query.terms.len()));
    if repository_matches > 0 {
        basis.push(format!("exact repository term ({repository_matches})"));
    }
    if exact_path {
        basis.push("exact path match".to_owned());
    }
    if basis.is_empty() {
        basis.push("lexical match".to_owned());
    }

    let confidence = if subject_matches > 0 && repository_matches > 0 {
        Confidence::High
    } else if subject_matches > 0 || repository_matches > 0 || covered > 1 {
        Confidence::Medium
    } else {
        Confidence::Low
    };

    RankedMaterial {
        material: Material {
            oid: candidate.oid.clone(),
            abbreviation: String::new(),
            subject: candidate.subject.clone(),
            paths: candidate.paths.clone(),
            confidence,
            basis,
        },
        score,
        commit_time: candidate.commit_time,
    }
}

fn compare_ranked(left: &RankedMaterial, right: &RankedMaterial) -> Ordering {
    right
        .score
        .partial_cmp(&left.score)
        .unwrap_or(Ordering::Equal)
        .then_with(|| right.commit_time.cmp(&left.commit_time))
        .then_with(|| left.material.oid.cmp(&right.material.oid))
}

fn changed_paths(connection: &Connection, oid: &str) -> Result<Vec<Vec<u8>>, AppError> {
    let mut statement = connection
        .prepare("SELECT old_path, new_path FROM changes WHERE commit_oid = ?1 ORDER BY ordinal")
        .map_err(|error| search_error("preparing changed paths", error))?;
    let rows = statement
        .query_map([oid], |row| {
            Ok((
                row.get::<_, Option<Vec<u8>>>(0)?,
                row.get::<_, Option<Vec<u8>>>(1)?,
            ))
        })
        .map_err(|error| search_error("reading changed paths", error))?;
    let mut paths = Vec::new();
    for row in rows {
        let (old_path, new_path) =
            row.map_err(|error| search_error("reading changed paths", error))?;
        for path in [old_path, new_path].into_iter().flatten() {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    Ok(paths)
}

fn assign_abbreviations(materials: &mut [Material]) {
    for index in 0..materials.len() {
        let oid = materials[index].oid.clone();
        if oid.is_empty() {
            continue;
        }
        let mut length = 12.min(oid.len());
        while length < oid.len()
            && materials.iter().enumerate().any(|(other, material)| {
                other != index
                    && material.oid.len() >= length
                    && material.oid[..length] == oid[..length]
            })
        {
            length += 1;
        }
        materials[index].abbreviation = oid[..length].to_owned();
    }
}

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

pub(crate) fn searchable_text(value: &str) -> String {
    let terms = tokenize(value);
    if terms.is_empty() {
        value.to_owned()
    } else {
        format!("{value}\n{}", terms.join(" "))
    }
}

fn analyze(input: &str) -> Query {
    let terms = tokenize(input);
    let paths = input
        .split_whitespace()
        .filter(|part| part.contains('/') || part.contains('\\') || part.contains('.'))
        .map(|part| normalize_path(part.as_bytes()))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    Query { terms, paths }
}

fn tokenize(input: &str) -> Vec<String> {
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
        if !terms.iter().any(|term| term == current) {
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

fn normalize_path(path: &[u8]) -> String {
    let path = String::from_utf8_lossy(path);
    let mut path = path.replace('\\', "/").to_ascii_lowercase();
    while let Some(stripped) = path.strip_prefix("./") {
        path = stripped.to_owned();
    }
    path.trim_matches('/').to_owned()
}

fn path_matches(query_path: &str, path: &[u8]) -> bool {
    let path = normalize_path(path);
    path == query_path || (!query_path.contains('/') && path.rsplit('/').next() == Some(query_path))
}

fn empty_report() -> SearchReport {
    SearchReport {
        materials: Vec::new(),
        matched_count: 0,
        truncated: false,
    }
}

fn search_error(operation: &str, error: impl std::fmt::Display) -> AppError {
    AppError::operational(format!(
        "error: {operation}: {error}; delete .gitscry and retry"
    ))
}
