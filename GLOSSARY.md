# GitScry

GitScry gives development agents access to relevant Git history while they work in a repository.

## Language

**cache**:
A rebuildable local representation derived from Git history and queried by an Agent. Git remains the source of truth.
_Avoid_: index, database, repository memory

**current change**:
The selected uncommitted additions, modifications, removals, and detected renames for which an Agent seeks historical material. It is distinct from the historical query scope.

**historical query scope**:
The subset of cache-reachable commits eligible to contribute material to a historical query. It may be narrowed by revision reachability and commit time without changing the query's target revision or implying coverage beyond the cache.

**historical detail**:
The contents of a cache-reachable commit or file version opened to inspect the context around returned material. It is not additional query-selected material and does not expand the historical query scope.

**material**:
Information returned from the cache that is traceable to its underlying commits, paths, or diffs and helps an Agent reason about a change.
_Avoid_: evidence, answer, recommendation

**suspect**:
A commit that may have introduced a regression, supported by material from its path, symbol, diff, or history.
_Avoid_: culprit, root cause

**file evolution timeline**:
A chronological account of commits affecting a file within cache-reachable history, including detected earlier paths. It does not claim that every entry is significant or that uncached history is covered.

**file incarnation**:
A file's continuous identity from its introduction through detected renames until its removal. A later file created at the same path is a separate incarnation.
