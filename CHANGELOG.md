## [0.4.1] - 2026-09-30

### 🐛 Bug Fixes

- *(update)* Defer Windows image cleanup
## [0.4.0] - 2026-09-30

### 🚀 Features

- *(search)* Add changed-line code search
- *(cli)* Add file evolution timeline
- *(search)* Add patch excerpts
- *(trace-fix)* Add attributed patch excerpts
- *(trace-fix)* Merge attributed patch excerpts
- *(search)* Scope queries by revision and time
- *(timeline)* Add patch excerpts
- *(timeline)* Merge patch excerpts
- *(why)* Attach relevant patch excerpts
- Merge target-linked patch excerpts
- Scope why and timeline history
- *(relations)* Scope co-change history
- *(scope)* Scope examples and failures
- *(scope)* Scope regression and trace-fix
- *(scope)* Scope regression and trace-fix

### 🐛 Bug Fixes

- *(search)* Bound patch excerpts
- *(search)* Rank scoped hits locally
- *(timeline)* Scope patch availability per change
- *(trace-fix)* Avoid false cache warning in scope

### 🚜 Refactor

- *(cache)* Bundle relation bindings
- *(scope)* Reduce query and enum size
- *(timeline)* Move into analysis
- *(cache)* Isolate writes and publication
- *(git)* Isolate symbol location
- Decouple citations from output format
- Deepen historical query execution
- Share history with patch selection

### 📚 Documentation

- *(domain)* Define file evolution timeline
- *(domain)* Define file incarnation
- Add gitscry rule in AGENTS.md
- Refine commit rule in AGENTS.md
- *(domain)* Define historical query scope
- Rename CONTEXT.md to GLOSSARY.md
## [0.3.1] - 2026-09-28

### 🐛 Bug Fixes

- *(package)* Exclude unused docs assets
## [0.3.0] - 2026-09-28

### 🚀 Features

- *(cli)* Add structured JSON query output

### 🐛 Bug Fixes

- *(cli)* Complete JSON query data

### 🚜 Refactor

- *(history)* Share line tracing
- *(cache)* Share snapshot row writes

### 📚 Documentation

- *(readme)* Add benchmark highlights
- *(readme)* Add benchmark test commands
- *(retrieval)* Describe line-history seam
- Sharpen README positioning and add real demos
- Add trace-fix example
- Add long gif example
- Add GitScry demo GIF to README
- Enhance image tag with alt attribute
- Invite feedback and issue reports
- Add contributing guide
- Add security policy
- Add intro vedio
- Add video preview card
- Show demo and overview video side by side
- Add thumbnail
- Use video thumbnail in README
- Remove obsolete video preview
- Add video in README
- Add README contents
- Move README contents before demo
- Fix issue read command
## [0.2.1] - 2026-09-24

### 🐛 Bug Fixes

- *(ci)* Allow manual crate publish
- *(ci)* Chain release publishing
- *(index)* Align type-change patch blocks
- *(trace-fix)* Align hunks after type changes

### ⚡ Performance

- *(cache)* Cap hunk buffers at 8 MiB
- *(cache)* Flush hunk metadata per block
- *(cache)* Box line dictionary keys
- *(cache)* Drop path maps before hunks
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
