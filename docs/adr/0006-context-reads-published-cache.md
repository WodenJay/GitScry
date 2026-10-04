# Context reads the published cache without refreshing it

Context queries are advisory, read-only views: they never populate, refresh, extend, or repair the published cache, whether or not `--max-followup-checks` is supplied. New history becomes available to context only after an explicit `gitscry index`; this keeps the opt-in relationship-check budget scoped to follow-up analysis instead of triggering separate cache-maintenance work. `context --hybrid` likewise requires compatible published semantic coverage and does not maintain it. This is the context-specific exception to ADR-0004.
