# GitScry

GitScry gives development agents access to relevant Git history while they work in a repository.

## Language

**cache**:
A rebuildable local representation derived from Git history and queried by an Agent. Git remains the source of truth.
_Avoid_: index, database, repository memory

**material**:
Information returned from the cache that is traceable to its underlying commits, paths, or diffs and helps an Agent reason about a change.
_Avoid_: evidence, answer, recommendation
