# Read Ownership

- Group cached reads by data responsibility, not by the CLI command that happens to consume them. Keep connection, lock, and published-generation ownership with `QuerySession`.
- Keep analysis policy with its analysis owner; cache reads may enforce storage/query constraints without deciding feature ranking or wording.
- Group similarly named reads by their semantics and data responsibility, not by naming alone.