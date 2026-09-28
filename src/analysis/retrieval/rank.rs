use std::{cmp::Ordering, collections::HashMap};

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
        .collect::<Vec<_>>();
    let abbreviations = unique_prefixes(oids);
    for material in materials.iter_mut() {
        for citation in &mut material.citations {
            citation.abbreviation = abbreviations
                .get(&citation.oid)
                .expect("citation object ID was collected")
                .clone();
        }
    }
}

fn unique_prefixes(mut oids: Vec<String>) -> HashMap<String, String> {
    oids.sort_unstable();
    oids.dedup();

    oids.iter()
        .enumerate()
        .map(|(index, oid)| {
            let mut shared_prefix = 0;
            if let Some(previous) = index.checked_sub(1).and_then(|index| oids.get(index)) {
                shared_prefix = shared_prefix.max(common_prefix_len(oid, previous));
            }
            if let Some(next) = oids.get(index + 1) {
                shared_prefix = shared_prefix.max(common_prefix_len(oid, next));
            }
            let prefix_length = 12.max(shared_prefix + 1).min(oid.len());
            (oid.clone(), oid[..prefix_length].to_owned())
        })
        .collect()
}

fn common_prefix_len(left: &str, right: &str) -> usize {
    left.bytes()
        .zip(right.bytes())
        .take_while(|(left, right)| left == right)
        .count()
}

#[cfg(test)]
mod tests {
    use super::unique_prefixes;

    #[test]
    fn prefixes_are_shortest_unique_and_deduplicate_oids() {
        let common = "0123456789abcdef";
        let first = format!("{common}{}", "0".repeat(24));
        let second = format!("{common}1{}", "0".repeat(23));
        let other = format!("fedcba9876543210{}", "0".repeat(24));
        let abbreviations = unique_prefixes(vec![
            first.clone(),
            second.clone(),
            other.clone(),
            second.clone(),
        ]);

        assert_eq!(abbreviations.len(), 3);
        assert_eq!(abbreviations[&first], first[..17]);
        assert_eq!(abbreviations[&second], second[..17]);
        assert_eq!(abbreviations[&other], other[..12]);
    }
}
