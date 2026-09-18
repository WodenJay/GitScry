# Evolution-tracing historical replay

> Throwaway prototype for [验证演化追踪能力](https://github.com/WodenJay/GitScry/issues/3). This is evidence for a product decision, not production design.

## Protocol

- Primary repository: `NousResearch/hermes-agent`, bare `--filter=blob:none` clone at `c661785f872b5647fbac7c138d965180783bd9af` (36,770 commits).
- Bounded stress repository: `rust-lang/rust`, bare `--shallow-since=2025-01-01` clone at `420ed2a0c3d7225b1744266fd884d431b4d8cfe0` (58,100 commits).
- Replay starts from a code location, symptom, or fix, then hides the known answer and reconstructs it from history.
- Compare with the obvious Git command; do not benchmark.
- Judge usefulness, traceability, advantage, and noise.
- Decision threshold agreed with the maintainer: one strong case is sufficient for `keep`.

## Verdicts

| Capability | Verdict | Decisive case |
|---|---|---|
| Code why / historical context (`why`) | **keep** | A harmless-looking leaf import encodes a repeatedly violated stale-process invariant and a production incident; blame and line log expose only fragments. |
| Regression localization assistance (`regress`) | **keep** | For a Matrix `/stop` regression, the latest path commit is the wrong suspect and no pre-fix failing test exists; function history and parent blame reduce four commits to the introducer. |
| Bug lineage / SZZ (`trace-fix`) | **keep** | A Rust compiler fix traces to a performance change in another PR despite bors merges and moved lines; parent deleted-line blame plus PR bridging recovers the verified lineage. |

## `why`: keep

### Strong primary case: why `cron.constants` must remain a leaf

Current location: `cron/occurrences.py:23`:

```python
from cron.constants import FIRE_CLAIM_SKEW_SECONDS
```

Plain blame points to `f967d9b8f11d7fbcd05286f3ffa11b51449ec597`, whose subject is only `fix(cron): isolate fire claim skew constant` and whose body is empty. The nearby arithmetic instead blames `fcbdcdb4281b2119c25b72517c88d0ffbed53591`, which explains clock-skew handling but not why the constant needs its own module.

```console
$ git blame -n -L 23,30 main -- cron/occurrences.py
f967d9b8f11 ... 23) from cron.constants import FIRE_CLAIM_SKEW_SECONDS
...
fcbdcdb4281 ... 29) # A skewed early fire ...
fcbdcdb4281 ... 30) earliest_real = ... FIRE_CLAIM_SKEW_SECONDS

$ git log --format='%h %s' --follow -- cron/constants.py
e4e63e325a fix(cron): move FIRE_CLAIM_TTL_SECONDS to the same leaf as the ske...
f967d9b8f1 fix(cron): isolate fire claim skew constant
```

The missing context is in [hermes-agent PR #114692](https://github.com/NousResearch/hermes-agent/pull/114692): a long-lived process could retain an old `cron.jobs` module while loading `cron.occurrences` from a newer tree. Importing the newly added constant from the stale module raised `ImportError`; a per-job handler swallowed it, silently skipping every enabled job until restart. The linked incident reported 115 error lines across five ticks and a missed daily delivery.

Related history shows the same stale-module trap had already caused repeated isolation work (`fd1d8271d`, `e24c8499f`, `576accd92`, `579dbe0c7`). The useful `material` is therefore not “this line was moved”: it is the repository-local invariant that fresh-loaded siblings must not import new names from long-lived cached modules, backed by exact commits, paths, and the incident PR.

**Compared with obvious Git:** blame returns a context-free move; `log -L` follows only the local line; `--follow` sees the two leaf-file commits. Reconstructing the reason requires joining multiple paths and the PR. The result is highly useful and traceable with low noise.

### Rust stress case

`compiler/rustc_parse/src/parser/function.rs` showed the same need under different adversity: file splitting stops blame at mechanical commit `0c1fefd9de9`, bors rollup merges have no useful patch, and first-parent blame attributes work to bors. Recovering the reason for the `extern "C" const unsafe fn` diagnostic required linking commit `acfca7a866c3fba220351ce6978f0e74d1c0ca0e`, PR #162204, issue #140171, and its regression fixtures. This strengthens `keep`, while showing that merge/PR and move boundaries must be explicit in any returned `material`.

## `regress`: keep

### Strong primary case: Matrix chat IDs break `/stop`

Fix:

- `f9e47df22ec5e7dd19e7a9282103a21e91f2ad62` — `fix(gateway): match the chat id as text so ids containing ':' survive`
- affected function: `_same_chat_key_slots` in `gateway/run_busy.py`
- symptom: a Matrix ID such as `!room:example.org` is itself colon-delimited, so splitting the whole session key makes both `/stop` fallbacks miss an active run.

Replay from the fix parent:

```console
$ git blame -L 49,54 f9e47df22^ -- gateway/run_busy.py
52f3fd5e779 ... 49) slots = key.split(":")
52f3fd5e779 ... 52) rest = slots[3:]
52f3fd5e779 ... 53) if rest[1] == chat_id:

$ git log --format='%h %s' \
    -L :_same_chat_key_slots:gateway/run_busy.py f9e47df22^
04c0bec905 fix(gateway): chat-scope match must not alias a chat_id or a reply...
52f3fd5e77 refactor(gateway): one structural matcher for the /stop fallback t...
```

The introducer is `52f3fd5e7798d434281c06ece87cbc5479434249`. Its diff replaced prefix matching, which tolerated colons inside the chat ID, with structural slot splitting. Its message says “no wiring change”, making it a credible accidental regression rather than a coincidental edit.

The candidate window from the prior good implementation to the fix contains four commits. Function history and parent blame reduce it to one introducer. By contrast:

- `git log -- gateway/run_busy.py` points first to `04c0bec905`, a nearby fix rather than the introducer.
- `git bisect` cannot start from the historical tree without a failing test: the Matrix regression test was added by the fix itself.
- A symptom without a path or symbol remains noisy; this capability needs at least one of those anchors.

The output is useful, fully traceable, and materially better than the obvious path log or an unavailable bisect. That single case meets the agreed `keep` threshold.

### Rust stress case and limit

Rust issue [#160994](https://github.com/rust-lang/rust/issues/160994) supplied an unusually good nightly range. Within its 57 commits, `git log -G impl_is_default` identifies `6c28fdda79b9d1be22c8da03342beb896b6b2898`; `-S` misses it because the predicate moved rather than changing count. Without the reporter-provided good/bad revisions, path history leaves 15–64 candidates. This case alone would merit `defer`, but it does not negate the independent Hermes case.

The capability should describe suspects and their evidence, not claim to replace executable bisecting.

## `trace-fix`: keep

### Strong stress case: Rust solver regression across PRs

Fix `39b6dbec0617230ba45954f27fd6ff4431b44674` says that the second commit of rust-lang/rust PR #160605 moved the `default impl` check later for performance and introduced issue #160994. The fix landed through PR #161268.

Blaming the lines removed by the fix in its parent gives the introducer directly:

```console
$ git blame -L 565,572 e702ecae8a3e -- \
    compiler/rustc_next_trait_solver/src/solve/assembly/mod.rs
6c28fdda79b ... 565) Ok(candidate) => {
...
6c28fdda79b ... 569) if !cx.impl_is_default(impl_def_id) {
...

$ git show -s --format='%H%n%s%n%b' 6c28fdda79b
6c28fdda79b9d1be22c8da03342beb896b6b2898
Reorder `assemble_impl_candidates` checks
`impl_is_default` is moderately expensive. This commit moves the cheaper
`consider_impl_candidate` check ... ahead, for a small perf win.
```

This is independently verified by [PR #161268](https://github.com/rust-lang/rust/pull/161268), which names PR #160605, the moved check, and its side effect as the regression cause.

The useful `material` is the auditable chain:

`#160605 / 6c28fdda79b` performance reorder → `#160994` ICE → `#161268 / 39b6dbec061` corrective reorder.

Plain blame supplies only the middle SHA. Producing the chain requires selecting deleted lines from the fix parent, distinguishing moved code (`-G` finds it while `-S` does not), and bridging the non-first-parent commit through bors merges to its PR. That is a meaningful advantage over a single obvious blame command.

### Failure modes observed

- Blame the **fix parent**, not HEAD; HEAD attributes added lines to the fix itself.
- Pure-add fixes have no deleted lines and need message/test corroboration.
- Code motion can make pickaxe identify a refactor instead of the behavior change.
- The Rust shallow clone absorbs pre-window lines into boundary commits; those attributions are unknown, not introducers.
- `.git-blame-ignore-revs` contained hashes absent from the shallow clone; Git accepted them without warning.
- Very short lineages remain useful but should carry lower confidence because they often only confirm what the fix message already says.

## Cost and scope notes

- No package was installed; only Git 2.55.0 and GitHub CLI 2.100.0 were used.
- Scratch clone sizes were 92 MiB for blobless Hermes and 281 MiB for shallow Rust.
- Blobless `-S`, `-L`, and `blame -C` can trigger on-demand network fetches; one search stalled for five minutes. Precomputed history data would be needed for predictable latency, but this prototype makes no architecture decision.
- Git-only history cannot recover PR bodies or cherry-picked source commits absent from local refs. Returned `material` must distinguish local facts, remote facts, and inference.
- This prototype validates candidate value only. Command contracts and production implementation remain for later wayfinding tickets.
