## [0.2.0] - 2026-09-22

### 🚀 Features

- *(update)* Add self-update command

### ⚡ Performance

- *(cache)* Carry change IDs into hunks
- *(retrieval)* Use hash set for token dedup
- *(cache)* Skip empty hunk setup
- *(index)* Stream patch ingestion

### 📚 Documentation

- Add release install method
- "8 cmd" in README.md
- Backfill v0.1.0 changelog
## [0.1.0] - 2026-09-22

### 🚀 Features

- Build initial Git cache
- *(search)* Add deterministic history search
- *(cache)* Add safe refresh lifecycle
- *(cli)* Add examples and failures history queries
- *(cli)* Add relation history queries
- *(why)* Add line history explanations
- *(regression)* Add suspect analysis
- *(trace-fix)* Trace fixes to introducing changes
- *(trace-fix)* Add fix lineage
- *(bench)* Add cache baseline harness
- *(cache)* Query published generations
- *(cache)* Compact v4 generation
- *(cache)* Compact cache payloads
- *(index)* Show terminal progress bar

### 🐛 Bug Fixes

- Honor index boundary cases
- *(cache)* Rehydrate partial history
- *(cache)* Detect stale Git objects
- *(cache)* Atomically replace Unix generations
- *(cache)* Validate refreshed search rows
- *(analysis)* Tighten failure and example precision
- *(analysis)* Stop inferring failure reasons
- *(relations)* Tighten co-change evidence
- *(tests)* Clarify missing-path warning
- *(render)* Preserve commit truncation labels
- *(regression)* Rank older suspects first
- *(trace-fix)* Bound degraded lineage
- *(tests)* Isolate every git command
- *(bench)* Harden baseline reporting
- *(bench)* Resolve Windows release binary
- *(failures)* Bound path queries
- Ignore submodule gitlinks as blobs
- *(cli)* Pin bin_name so usage shows gitscry, not the binary path
- *(retrieval)* Reject Windows drive paths

### ⚡ Performance

- *(relations)* Use indexed path projection
- *(failures)* Reuse path projection
- *(index)* Scan blocked commits once

### 🚜 Refactor

- *(analysis)* Split retrieval, provenance, capabilities
- *(cache)* Centralize memory opening
- *(cache)* Own generation lifecycle
- *(history)* Unify regression sources
- *(tests)* Share repository harness
- *(cache)* Own history queries

### 📚 Documentation

- Define cache and material
- Select prototype repositories
- Remove timeout in AGENTS.md
- Record issue-15 historical replay
- *(relations)* Record bounded replay
- Remove temp issue doc
- Define regression suspects
- *(bench)* Record Hermes cache results
- *(benchmark)* Report path projection size
- *(cli)* Explain intent, inputs, and examples in help
- *(cli)* Drop intent bullets duplicated by command list
- Refine reviewer rule in AGENTS.md
- Add logo and README template
- Refine README.md
- Complete release README
- Add agent install guide
