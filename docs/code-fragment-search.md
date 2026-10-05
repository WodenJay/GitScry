# Exact historical fragment search

`gitscry search --code-file PATH` reads a byte-valued fragment from a file;
`--code-file -` reads stdin through EOF. This mode is exclusive with the positional
query, `--code`, and `--code-regex`. Existing code-search path, direction, scope,
limit, and optional GitHub association options apply; hybrid retrieval does not.

One or more whole lines must match added or removed material within one cached
hunk. LF and CRLF terminators are equivalent for matching only. Other bytes,
including indentation, trailing whitespace, BOMs and standalone CRs, are literal.
One final terminator does not add a blank query line. Context breaks continuity;
opposite-direction changes do not. Occurrences can repeat or overlap. Limits
count occurrences and do not stop the scan. These results are historical material,
not evidence of semantic identity, replacement, intent or causation.

## JSON contract

Fragment mode selects `schema_version: 6` and `kind: "code-fragment-search"`.
Existing single-line modes retain their existing schema selection and fields.

The envelope contains `scope`, `matched_count`, `truncated`, `occurrences`,
`warnings`, and `github_links` (null when not requested). Scope uses the shared
historical scope and cache-coverage contract. `truncated` means the result limit
excluded occurrences, independently of surrounding-material truncation.

Each occurrence contains:

- `commit_id`: full historical commit ID.
- `path`: new historical path for additions, old historical path for removals.
- `direction`: `added` or `removed`.
- `start_line`, `end_line`: inclusive historical source coordinates.
- `content`: complete historical source bytes, without diff prefixes. Original
  line endings and absence of a final newline are retained.
- `surrounding`: default patch material, enclosing the matched diff rows and up
  to three preceding/following diff rows within the same hunk. It contains
  `old_path`, `new_path`, `old_hunk_start`, `new_hunk_start`, `first_diff_row`,
  `last_diff_row` (zero-based inclusive hunk row offsets), `text`, `status`,
  `truncated`, and `unavailable`. Status is `available`, `truncated`, or
  `unavailable`. Presentation budgets affect only surrounding text, never match
  content or locators. Text is null when unavailable.

Byte-valued paths, content, and surrounding text use the existing UTF-8 string
or `{ "base64": "..." } representation. No cache-format
migration, full-file retrieval, implicit timeout, or rename following is added.
