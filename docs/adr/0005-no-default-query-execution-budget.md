# Do not impose a default query execution budget

GitScry does not impose an implicit execution budget or timeout on queries merely to protect users from potentially long runtimes. Users can stop a query they consider too slow; silently stopping work on their behalf would trade historical coverage for an assumed latency preference. Queries should provide an optional execution-budget parameter, applied only when the user explicitly sets it, and clearly report when that budget limits coverage.

This decision concerns execution cutoffs, not the requested historical query scope, relationship observation windows, result limits, or defined sampling rules. Those define what a query examines or returns; they must not be used as undisclosed substitutes for a default execution budget. A user-requested cutoff must not present unexamined history as absence of a relationship.
