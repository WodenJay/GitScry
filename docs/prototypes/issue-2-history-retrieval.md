# Historical retrieval capability replay

Issue: [验证历史检索能力](https://github.com/WodenJay/GitScry/issues/2)

## Protocol

Replay used the full `main` history of `NousResearch/hermes-agent` from a bare
`--filter=blob:none` clone. Ordinary changes were read against their first
parent. Each capability used no more than two Hermes cases. One bounded
`rust-lang/rust` commit-message case reviewed the promising failed-approach
result.

The baseline was the obvious Git command (`git log --grep`, path-scoped
`git log`, or `git log -S`), not a benchmark. Results were judged by usefulness,
traceability, advantage over that baseline, and noise.

## Verdicts

| Capability | Verdict | Why |
|---|---|---|
| Historical semantic search (`history search`) | **keep** | Concept-bearing queries jointly rank related changes that exact grep scatters through hundreds of hits. Exact grep remains preferable when the repository term is already known. |
| Historical modification examples (`examples`) | **keep** | Related diffs expose repeatable migration checklists that path history obscures and `-S` cannot find unless the caller already knows the symbol. |
| Revert / failed-approach memory (`failed`) | **keep** | Pairing an abandoned change with its reason and later constraints prevents repeating approaches; raw revert grep is too noisy and often cannot identify the reverted work. |

## Historical semantic search

### Concurrent scheduled-job execution

Query intent: prevent two processes from running the same scheduled job.

Useful first-parent commits included:

- `06cc983b86516344f7a6bc486ae8e0838ebaab09` — prevent double execution of
  one-shot jobs across concurrent schedulers.
- `3b5c645433a87960eb5e5c2d124930548b90797f` — durable run claim instead of
  a fixed time advance.
- `acaafcc6bba3f6fa1af0702ce986e0c4f79ed0c6` — make immediate execution
  race-safe.
- `cd537187611769ebb6a1aa9460265e9ef5694606` — prevent long-running scripts
  from running twice.

A subject-weighted concept query ranked the first two at positions 1 and 3.
Single-term baselines such as `git log -i --grep=double`, `--grep=concurrent`,
and `--grep=claim` produced 529, 609, and 954 hits respectively; the related
commits appeared hundreds of results down. Natural-language retrieval weakened
when the query omitted repository terms such as `claim`, `dedupe`, and `race`.

### Secret redaction

Query intent: prevent an API key or secret from leaking into log output.

The useful family included `57b48a81c`, `fb634068d`, `b41eee450`, and
`de928bccd`. A concept query placed three family members in its top 15. Exact
`--grep=redact`, `--grep=secret`, and `--grep=leak` returned 339, 880, and 1,025
hits and buried the same changes.

**Decision constraint:** keep exact Git grep beside concept search. Semantic
ranking helps when the Agent knows the symptom but not the repository noun; it
must not replace precise search.

## Historical modification examples

### Removing a provider while preserving migration guidance

- `998f614c7f8e066728dd6c8e399916f44eb4f238` removes the keyless
  `opencode-free` tier across provider registration, aliases, model catalog,
  authentication, tests, and compatibility manifests while retaining a user
  migration hint.
- `d6773cf26fab5457005297fb39bedfd18842ae40` removes the Tavily backend and
  configuration surfaces while deliberately retaining redaction for legacy
  `tvly-` keys.

Together they provide a reusable checklist: deregister every surface, migrate
fixtures, update compatibility metadata, preserve legacy-secret redaction, and
name the replacement. Path history returns thousands of unrelated provider
changes; `git log -S<provider>` requires knowing the removed symbol first.

### Excluding rows from an FTS index

- `ea65fcd980a7b7031b43cc4b45a33600b7c0a03f` excludes cron sessions.
- `5a7bee0fa8970f53d4617b32fa58c7f8c8028bad` excludes tool calls and reuses the
  deployed-layout rebuild path.
- `2b55ded1ac5f3b41cdc580974e745631dac1bb53` excludes delegate-child transcripts,
  updates the shared predicate and triggers, advances the migration gate, and
  purges old postings once on open.

The material is a concrete migration recipe rather than a recommendation:
extend the shared admission predicate, update every trigger/backfill path,
advance the schema gate, and clean existing postings. Path history for
`hermes_state_schema.py` contained 66 mostly unrelated commits.

## Revert / failed-approach memory

### macOS TCC interpreter anchor

- Revert: `2f9e18700159ba5df1ec8a69e8e5a2e7ceb368e9`.
- Earlier implementation: `cc5ff96f9e6af389e74c3337c25fbf4b78a090bf`,
  later extended by `5259f565d0e373fd0bb769eb8fcfca2d7efb9bf3` and
  `9f8cdf89d6319e17d155dd7c2c77175abe30e376`.

Copying the uv interpreter into `venv/bin` made `LC_RPATH` resolve against a
`venv/lib` without `libpython`, bricking commands on real macOS hardware. Fake
Linux test interpreters could not expose the failure. The revert records the
safe retry condition: bundle or link `libpython` (or rewrite the rpath), verify
on macOS hardware, and run a pre-install boot gate. The corrected re-land
`aa72df4b426891d9dc1d92ee4f4d95773ace29cb` followed those constraints.

### Shared cumulative-resend heuristic

- Revert: `2b5268f716c2a69ad451de0baed57138191ebebb`.
- Reverted change: `ca03486b6a5a86e2be28d83d4cad61770619e7fb`.

The shared streaming path treated a longer fragment beginning with the previous
buffer as a cumulative resend and replaced the buffer. Ordinary incremental
fragments can have the same shape, so the heuristic corrupted tool-call
arguments. The recorded retry condition is to gate handling to the affected
provider rather than guess on every provider's shared path.

`git log --all --grep=revert` returned 555 commits, but only 95 had a subject
beginning with `Revert`/`revert`; 31 of those lacked a standard
`This reverts commit <sha>` trailer. Both cases above require reading prose and
history rather than mechanically parsing the standard trailer.

### Bounded rust-lang/rust review

`315ecf4a939def16631c2b25c3782ad67fc22160` reverts type-operation region
constraint preservation because it imposed work on a frequently called path
even when assumptions-on-binders was disabled. Its message records the safe
retry: guard the new logic, then obtain real performance numbers. The reason is
useful material, while first-parent `--grep=revert` is polluted by rollup merge
messages that merely list reverted PRs.

## Operational caveat

In a `--filter=blob:none` partial clone, `git log -S` may fetch missing blobs and
can fail mid-walk. A partial result must not be presented as complete. Commit
message and first-parent metadata walks remained offline-safe; selected diffs
were read only after their blobs were available.
