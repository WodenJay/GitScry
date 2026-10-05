//! Adapt content-origin observations to context material without pooling path origins.
use super::{Category, HistoricalFollowup, HistoricalFollowupChain, Suggestion};
use crate::analysis::retrieval::follow_on::{self, Origin, Origins};
use crate::{
    app::AppError,
    cache::{QuerySession, SearchFilter},
    git::{CurrentChange, Repository},
};
pub(crate) use follow_on::Coverage;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn discover(
    session: &QuerySession,
    repository: &Repository,
    input: &CurrentChange,
    content_origins: &[(usize, i64, Suggestion)],
    observation_days: usize,
    scope: Option<&SearchFilter>,
    coverage: &mut Coverage,
) -> Result<Vec<(usize, i64, Suggestion)>, AppError> {
    let Some(revision) = input
        .head
        .as_deref()
        .or(scope.map(|scope| scope.to_oid.as_str()))
    else {
        return Ok(Vec::new());
    };
    let mut events = BTreeMap::<String, Origin>::new();
    for (strength, time, suggestion) in content_origins {
        let Some(citation) = suggestion.citations.first() else {
            continue;
        };
        let origin = events
            .entry(citation.oid.clone())
            .or_insert_with(|| Origin {
                oid: citation.oid.clone(),
                strength: *strength,
                time: *time,
                associated_paths: BTreeSet::new(),
                path: suggestion.path.clone(),
            });
        origin.strength = origin.strength.max(*strength);
        origin.time = origin.time.max(*time);
        origin
            .associated_paths
            .extend(suggestion.associated_current_paths.iter().cloned());
    }
    let observations = follow_on::discover(
        session,
        repository,
        Origins {
            revision,
            selected_paths: input.paths().into_iter().collect(),
            events: events.into_values().collect(),
            require_current_file: true,
        },
        observation_days,
        scope,
        coverage,
    )?;
    Ok(observations.into_iter().map(|observation| {
        let undisplayed_supporting_origins = observation.supporting_origins.saturating_sub(observation.chains.len());
        let mut basis = vec![
            "same-file historical association through detected renames; not a causal conclusion".to_owned(),
            format!("proper descendants within {observation_days} days and {} parent edges; merge diffs do not count as support", follow_on::MAX_PARENT_DISTANCE),
            format!("supporting content origins: {}/{}", observation.supporting_origins, observation.eligible_origins),
            format!("candidate baseline: {}/{} sampled complete origins", observation.baseline_occurrences, observation.baseline_sample_size),
        ];
        if undisplayed_supporting_origins > 0 {
            basis.push(format!("{} additional supporting origins not shown as chains", undisplayed_supporting_origins));
        }
        let detail = HistoricalFollowup {
            supporting_origins: observation.supporting_origins,
            complete_origins: observation.eligible_origins,
            independent_chains: observation.independent_chains,
            baseline_occurrences: observation.baseline_occurrences,
            baseline_sample_size: observation.baseline_sample_size,
            undisplayed_supporting_origins,
            chains: observation.chains.into_iter().map(|chain| HistoricalFollowupChain { origin_oid: chain.origin_oid, later_oid: chain.later_oid }).collect(),
            observation_days, basis: basis.clone(),
        };
        (observation.independent_chains, observation.latest_support_time, Suggestion {
            category: Category::HistoricalFollowup, path: observation.path,
            associated_current_paths: observation.associated_current_paths, basis,
            selection_routes: vec!["historical_followup"], citations: Vec::new(),
            supporting_count: observation.supporting_origins, citations_truncated: false,
            content_matches: Vec::new(), content_matches_truncated: false,
            abandonment: None, historical_followup: Some(detail), co_change: None,
            follow_on: Vec::new(),
        })
    }).collect())
}
