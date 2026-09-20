# Cache and CLI baseline

This is the reproducible before/after harness for [#23](https://github.com/WodenJay/GitScry/issues/23), extending the cache measurements from [#22](https://github.com/WodenJay/GitScry/issues/22).
It measures a release binary against the two validation repositories selected in
[`validation-repositories.md`](../research/validation-repositories.md): `medium` is
`NousResearch/hermes-agent`, and `large` is `rust-lang/rust`.

## Behavior contract

The CLI contract is a black-box integration test over one deterministic Git fixture. It runs
`search`, `examples`, `failures`, `related`, `tests`, `why`, `regression`, and `trace-fix`, then
compares status plus escaped stdout/stderr byte-for-byte. The contract intentionally observes
rendered material, ordering, confidence, citations, provenance, and escaping; it does not inspect
SQLite tables or indexes.

```bash
cargo test --test cli_contract --jobs 1
```

If an intentional behavior change is approved, regenerate the fixture and review the diff:

```bash
GITSCRY_UPDATE_CLI_CONTRACT=1 cargo test --test cli_contract --jobs 1
```

## Release benchmark

Build the binary outside the benchmark so build time is not included:

```bash
cargo build --release --jobs 1
```

Clone the validation repositories as ordinary, full-history worktrees. The probe paths below are
known to exist in the selected repositories:

```bash
git clone https://github.com/NousResearch/hermes-agent.git ../validation/hermes-agent
git clone https://github.com/rust-lang/rust.git ../validation/rust
```

Run at least five measured samples per command/state. The script performs one untimed process
warm-up first. Cold mode drops the filesystem cache before the warm-up and before every measured
sample; warm mode leaves the filesystem cache alone. `index` removes `.gitscry` before each sample,
so its result is a rebuild measurement. The ordinary commands use the published cache left by the
last index run.

The report also measures `query_session_setup`: a warm, cache-only startup probe that sets the internal
`GITSCRY_BENCHMARK_QUERY_SETUP=1` benchmark switch. It opens the published cache session and exits
before capability analysis, so its timing excludes target-specific Git work and is the measurement used
for the issue's 100 ms setup gate.
On Linux, the default cold-cache hook writes `/proc/sys/vm/drop_caches` and therefore needs the
usual privilege. On Windows or macOS, pass the host's approved privileged cache-drop command;
the harness refuses to label an uncontrolled run as cold.

```bash
python scripts/cache_baseline.py \
  --binary target/release/gitscry \
  --repo medium=../validation/hermes-agent \
  --repo large=../validation/rust \
  --runs 5 \
  --cold-command "<privileged filesystem-cache drop command>" \
  --output benchmark-results/cache-baseline.json
```

A repository with a different checkout can override its command probe without changing the
benchmark protocol:

```bash
python scripts/cache_baseline.py \
  --repo medium=/path/to/hermes-agent \
  --repo large=/path/to/rust \
  --probe medium=gateway/run.py \
  --probe large=library/alloc/src/rc.rs
```

## Report contents

`scripts/cache_baseline.py` emits JSON to stdout and optionally writes the same report to
`--output`. Each repository records its HEAD, commit count, Git version, probe path, and measured
cache after indexing. Cache size is the total `.gitscry` bytes; the SQLite `dbstat` report lists
physical table/index objects for diagnosis only. The report also records:

- `index` wall-time median/p95 for cold and warm filesystem-cache states;
- the warm cache-only `query_session_setup` median/p95 used for the 100 ms setup gate;
- every ordinary command's cold and warm wall-time samples, median, and p95;
- the maximum sampled peak resident set size for each scenario;
- return codes and SHA-256 hashes of stdout/stderr, so unstable output is visible;
- platform, Python, binary, sample count, timeout, cold-cache method, and reproduction command.

p95 uses the nearest-rank sample (`ceil(0.95 * n)`), so five samples report the slowest sample as
p95. Timing and memory are release-benchmark measurements, not wall-clock assertions in ordinary
unit/integration tests.

## Before reference

The historical reference from [#21](https://github.com/WodenJay/GitScry/issues/21) is kept in
the JSON report beside new measurements: approximately 38,306 commits, 1.45 GB cache, 4.5 s
ordinary query preparation, and known slow cases of 31.8 s for `failures`, 61.5 s cold for
`related`, and 13.1 s cold for `tests`. This reference corpus is intentionally labelled separately
from the `large` rust validation checkout: the report never presents cross-corpus values as a
before/after comparison. The values are approximate historical measurements, not fabricated from
the current checkout.
