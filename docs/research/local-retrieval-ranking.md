# Local retrieval and ranking

Status: research note for [选择本地检索、排序与 embedding 方案](https://github.com/WodenJay/GitScry/issues/8).

## Decision

Use bundled SQLite FTS5 for lexical candidate retrieval and ordinary SQLite tables for path/co-change and lineage signals. Rank every capability with deterministic, capability-specific rules. Do not ship embeddings or Tantivy in the MVP.

This keeps retrieval inside the SQLite cache already selected by the cache contract, works offline, and leaves Git as the source of truth. The CLI does not expose the mechanism.

## Evidence

### Primary-source facts

- FTS5 provides BM25 ranking, per-column weights, Unicode tokenization, external-content tables, and rebuild support in SQLite itself: <https://sqlite.org/fts5.html>.
- `rusqlite`'s bundled SQLite enables FTS5: <https://github.com/rusqlite/rusqlite/blob/master/libsqlite3-sys/build.rs>.
- Tantivy is a Rust-native full-text engine with BM25, tokenizers, and a segment-based index: <https://github.com/quickwit-oss/tantivy>.
- Tantivy's default analyzer includes `RemoveLongFilter::limit(40)`, so a GitScry integration would need a repository-appropriate analyzer rather than the default: <https://docs.rs/tantivy/0.25.0/src/tantivy/tokenizer/tokenizer_manager.rs.html>.
- `fastembed` uses ONNX Runtime and Hugging Face model retrieval. Its default features include runtime/model download paths; callers can instead provide their own model files: <https://github.com/Anush008/fastembed-rs>.
- ONNX Runtime is an additional native runtime with platform-specific distribution artifacts: <https://onnxruntime.ai/docs/install/>.

### Bounded local measurements

These are architecture probes, not a benchmark. They were run on one Windows host against 37,207 commits from the primary validation repository, `NousResearch/hermes-agent`.

| Probe | Result |
|---|---:|
| FTS5 three-field message index | 2.6 s build, ~134 MB database including content |
| Tantivy 0.25 three-field index without stored text | 1.4 s, 32.6 MB |
| FTS5 top-10 query | 0.2–29 ms across tested terms |
| Tantivy top-10 query | 0.03–0.7 ms |
| MiniLM int8 embedding pass | 478 s at 77.8 documents/s |
| 384-dimension vectors | 14–57 MB depending on representation |

Eight small gold sets were taken from the accepted historical-replay prototypes. Lexical BM25 produced mean reciprocal rank 0.348; MiniLM embeddings produced 0.216. Combining their top-10 results recovered no additional gold commit in those eight cases. This sample is too small for a general quality claim, but it provides no evidence that embeddings repay their packaging, indexing, or offline costs for the MVP.

Both FTS5 and Tantivy produced tied scores whose default order depended on physical layout. The CLI contract therefore requires an explicit final order of score descending, commit time descending, then SHA ascending regardless of engine.

Repository terminology mattered more than paraphrase in the replay cases. Subject/body/path weighting and identifier/path terms recovered the useful commits. The one vague query helped by embeddings lacked repository nouns; deterministic repository-term expansion is the cheaper MVP response, but remains an implementation hypothesis to validate.

## Rejected alternatives

### Embeddings in the MVP

Rejected because they add a model, tokenizer, ONNX Runtime, model lifecycle, and offline packaging path without improving recall in the bounded replay. Default `fastembed` behavior is incompatible with GitScry's prohibition on implicit network access unless runtime and model artifacts are supplied explicitly.

Do not persist empty embedding columns or expose embedding flags for possible future use. Preserve only the natural seam between candidate generation and ranking. Reconsider embeddings after a larger replay set demonstrates lexical recall failures that deterministic term expansion cannot fix.

### Tantivy

Rejected despite lower index size and latency. At the measured scale, FTS5's worst tested query remained below 30 ms. Tantivy would introduce a second persisted format, index publication lifecycle, recovery path, and dependency set alongside the SQLite cache selected in issue 7. That complexity buys no user-visible MVP benefit.

### Trigram or stemmed primary tokenization

Rejected. Trigram indexing more than doubled the measured FTS5 footprint and replaces token search with substring behavior. Porter stemming changes repository identifiers and had no meaningful measured footprint advantage. Use `unicode61` with positions retained.

## Shared retrieval pipeline

1. **Analyze input deterministically.** Keep revisions and paths outside the text query. Split prose, path segments, issue references, and identifier subtokens. Remove stop words only from prose. Quote every term before constructing FTS5 `MATCH`; punctuation such as `.`, `/`, `#`, `+`, `-`, and parentheses otherwise changes or invalidates FTS5 syntax.
2. **Generate a bounded candidate pool.** Use weighted FTS5 over commit subject, body, and changed paths for textual capabilities. Use path-touch, co-change, first-parent, blame, and revert relationships for structural capabilities. Fetch more than the requested limit only by a fixed multiplier.
3. **Collect traceable facts.** Attach the commits, paths, hunks, lineage edges, and matched terms that can become `basis:` output.
4. **Rank with capability-specific signals.** Normalize lexical rank before combining it with structural signals. Use fixed weights and fixed-point values where practical. Scores are internal and never printed.
5. **Order deterministically.** Sort by score descending, commit time descending, then full SHA ascending.
6. **Bound and degrade.** Apply `--limit`, report truncation, lower confidence when required objects or history are unavailable, and never infer missing rationale.

Use an FTS5 external-content table derived from the authoritative cache rows. Subject should carry the highest BM25 column weight, body next, changed paths lowest. Initial replay supports a 10:3:2 subject/body/path starting point; weights remain an internal implementation detail and should be adjusted only against historical replay.

Confidence is independent of rank:

- **high**: direct local-history fact corroborated by an independent diff, blame, lineage, or co-change fact;
- **medium**: multiple consistent indirect facts;
- **low**: one fact, incomplete local objects, or a shallow, move, rename, or merge boundary.

## Capability rules

| Command | Candidate source and ranking signals | Required filtering and degradation |
|---|---|---|
| `search` | Weighted BM25; subject match; query-term coverage; exact repository-term/path match | Return the fixed no-result text. A term-expansion pass may run only when initial material is weak and must lower confidence. |
| `examples` | BM25 plus optional path overlap; prefer coherent multi-path changes; demote reverted changes | Historical paths are allowed. Describe reusable steps, never mandates. |
| `failures` | Revert subject/trailer relationships first, then lexical failure material; prefer a locally resolvable reverted commit, stated reason, and later corrective history | A missing stated reason is `Reason unknown`. An unresolved trailer or missing object lowers confidence. |
| `why` | Start from line/symbol history, then include co-changed paths and explanatory commit bodies; mechanical move/refactor commits are weak unless corroborated | Path and anchor validity follow the CLI contract. Expose move, rename, merge, submodule, and shallow boundaries. |
| `regression` | Restrict path history to the requested good/bad range; use symbol/hunk overlap, temporal position, and relevant test history | Results remain suspects. Without direct hunk support, confidence cannot be high; never imply replacement for `git bisect`. |
| `trace-fix` | Inspect lines deleted by the fix in its parent; combine blame, first-parent/merge lineage, pickaxe-style term movement, and message references | Pure-add fixes, code motion, unresolved merges, shallow history, or missing blobs explicitly degrade. Never invent an introducer. |
| `related` | Co-change count, proportion of seed touches, candidate ubiquity, and latest supporting commits | Exclude mass-change commits from relation statistics. Deduplicate multiple seeds. Never say a path must change. |
| `tests` | The same co-change signals, restricted to test-shaped paths; reward support beyond obvious mirrored-name matches | Returned tests must exist in the current worktree. Drop stale paths and warn when rename continuity is unresolved. |

`why`, `regression`, and `trace-fix` should not search every diff hunk lexically. They first narrow candidates structurally, then inspect cached hunks for those commits. A whole-history hunk FTS index is deferred: a 5,000-commit sample produced 281,120 chunks and showed that hunk indexing would dominate retrieval storage without being required by the accepted replay cases.

## Implementation constraints for the architecture decision

- Retrieval must consume one pinned, complete cache generation.
- FTS rows and structural ranking data are derived and rebuilt with that generation; they are not another source of truth.
- The architecture needs one candidate type carrying commit identity, source signals, supporting facts, confidence inputs, and deterministic sort keys.
- Capability orchestration owns filters and signal weights; the FTS layer owns only safe query construction and lexical candidates.
- Rendering receives ranked material and its basis, never raw engine scores.
- No network-capable model initialization belongs in the MVP query path.

## Known limits

- The quality comparison uses only eight replay-derived gold sets from one primary repository.
- Repository-term expansion was not prototyped; it is a lower-cost next experiment, not a proven feature.
- Tokenization for CJK prose remains weak with `unicode61`; current repository evidence does not justify trigram indexing. Revisit only with repositories where CJK commit material is common.
- The initial BM25 weights and mass-change threshold require implementation-time replay checks; neither is part of the CLI contract.
