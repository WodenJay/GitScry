use std::cmp::Ordering;

use super::super::Material;

/// A candidate with a capability-specific score attached.
pub(in crate::analysis) struct Ranked<T> {
    pub(in crate::analysis) value: T,
    pub(in crate::analysis) score: f64,
    pub(in crate::analysis) commit_time: i64,
    pub(in crate::analysis) oid: String,
}

/// Strongest first, then newer commits, then full object ID ascending.
pub(in crate::analysis) fn sort<T>(ranked: &mut [Ranked<T>]) {
    ranked.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| right.commit_time.cmp(&left.commit_time))
            .then_with(|| left.oid.cmp(&right.oid))
    });
}

/// Give every commit cited by the report the shortest unique prefix, at least 12 characters.
pub(in crate::analysis) fn assign_citations(materials: &mut [Material]) {
    let oids = materials
        .iter()
        .flat_map(|material| material.citations.iter())
        .map(|citation| citation.oid.clone())
        .collect::<Vec<String>>();
    for material in materials.iter_mut() {
        for citation in &mut material.citations {
            citation.abbreviation = unique_prefix(&oids, &citation.oid);
        }
    }
}

fn unique_prefix(oids: &[String], oid: &str) -> String {
    if oid.is_empty() {
        return String::new();
    }
    let mut length = 12.min(oid.len());
    while length < oid.len()
        && oids
            .iter()
            .any(|other| other != oid && other.len() >= length && other[..length] == oid[..length])
    {
        length += 1;
    }
    oid[..length].to_owned()
}
