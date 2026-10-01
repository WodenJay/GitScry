# Search JSON count fields

`gitscry search --json` returns a `matched_count`, `truncated`, and `notices` field.

For ordinary lexical search, `matched_count` retains the existing lexical-search meaning. With `--hybrid`, it is the number of distinct commits in the union of the lexical and semantic candidate lists after deduplication. Each list is bounded to `max(100, --limit)` eligible candidates, so this count is not a corpus-wide total of relevant commits.

`truncated` is true when `matched_count` is greater than the requested `--limit`; in hybrid mode, it describes unreturned candidates in that bounded union, not additional unseen corpus entries. The `notices` field explains the hybrid candidate depth. Historical revision and time scope is applied before either branch selects its candidates.
