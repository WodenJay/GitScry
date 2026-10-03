## [0.8.0] - 2026-10-03

### 🚀 Features

- *(search)* Add regex code search
- *(stats)* Add local usage stats
- Add shared cache clear
- *(index)* Index current HEAD history
- *(index)* Merge current HEAD indexing
- *(semantic)* Scope readiness to history views
- *(cache)* Prune missing-object history
- *(cache)* Merge missing-object pruning
- *(cli)* Group query and management help

### 🐛 Bug Fixes

- *(cache)* Share cache across worktrees
- *(cache)* Wait on cache publication
- *(query)* Scope history to current HEAD
- *(cache)* Batch missing-object lookup
- Normalize Windows-style path filters
- Preserve clear preview metadata
- *(cache)* Combine merged test modules
## [0.7.0] - 2026-10-03

### 🚀 Features

- *(why)* Track symbol name continuity
- *(why)* Follow verified symbol relocations
- *(why)* Merge symbol continuity tracking
- *(context)* Discover co-changing paths
- *(context)* Merge change-path discovery
- *(context)* Verify changed-code history
- *(context)* Merge changed-code history
- *(search)* Add CJK lexical retrieval
- *(search)* Add CJK lexical retrieval
- *(why)* Integrate partial symbol history
- *(search)* Prioritize complete CJK matches
- *(context)* Attach recorded abandonments
- *(context)* Merge recorded abandonments
- Add bounded same-file followups
- *(followups)* Merge bounded same-file query
- Discover cached deletion events
- Merge trace-removal discovery
- *(context)* Gate explicit hybrid resources
- *(context)* Merge verified local hybrid material
- *(context)* Merge local hybrid retrieval
- *(related)* Add concrete-path patterns
- *(related)* Merge concrete-path patterns
- *(hotspots)* Rank cached file incarnations
- *(hotspots)* Merge cached file ranking
- *(followups)* Follow detected renames
- *(followups)* Merge rename continuity
- *(followups)* Show revert references
- *(followups)* Merge revert references
- *(related)* Track file incarnations
- *(related)* Merge file incarnations
- *(trace-removal)* Add bounded patch context
- *(trace-removal)* Merge bounded patch context
- *(trace-removal)* Add same-commit navigation
- *(trace-removal)* Merge same-commit navigation
- *(hotspots)* Filter history and directories
- *(hotspots)* Report textual churn
- *(hotspots)* Merge textual churn
- *(followups)* Track changed regions
- *(followups)* Merge changed-region tracking

### 🐛 Bug Fixes

- *(why)* Reject uncertain symbol replacements
- *(context)* Validate dates for clean input
- *(context)* Enforce budgets and exact text
- *(why)* Preserve partial symbol history
- *(retrieval)* Require revert ancestry
- *(retrieval)* Require follow-up ancestry
- *(retrieval)* Reject parallel-branch history links
- Bound retained deletion-event evidence
- *(context)* Retain empty hybrid request flag
- *(patterns)* Bound mining and disclose coverage
- *(hotspots)* Join merge parents by incarnation
- *(followups)* Accept unambiguous OID prefixes
- *(trace-removal)* Correct excerpt starts
- *(followups)* Preserve seams and cap work

### ⚡ Performance

- *(context)* Query matched commit positions

### 🚜 Refactor

- *(analysis)* Colocate feature workflows
- *(cache)* Group reads by responsibility
- *(git)* Group target preparation by feature
- Colocate removal material assembly

### 📚 Documentation

- Avoid agent changing README
- Add docs rule
- Guide agents on code placement
## [0.6.0] - 2026-10-02

### 🚀 Features

- *(github)* Link other material queries
- *(github)* Link code and timeline results
- *(github)* Merge code and timeline links
- *(github)* Add PR issue associations
- *(github)* Merge PR issue associations
- *(why)* Separate attribution and modifications
- *(why)* Add factual history attribution
- *(why)* Trace symbol history
- *(why)* Merge symbol history
- *(github)* Paginate association coverage
- *(github)* Merge coverage-first pagination

### 🐛 Bug Fixes

- *(index)* Resolve sole local branch
- *(git)* Normalize target paths
- *(query)* Default targets to cache tip
- *(why)* Preserve uncertain symbol changes
- *(github)* Preserve links at object limit
- *(regression)* Trace before filtering material
- *(tests)* Compile Unix GitHub link checks
- *(tests)* Delimit fake gh responses with newlines
- *(tests)* Align GitHub issue fetch expectations
- *(release)* Publish drafts and dispatch crates.io

### ⚡ Performance

- *(trace-fix)* Batch deleted-line reads

### 🚜 Refactor

- *(render)* Narrow generic report kinds
- *(github)* Own association lookup state
- Merge architecture deepening

### 📚 Documentation

- *(cli)* Clarify timeline association coverage
- *(cli)* Clarify association budgets
- Define historical change patterns
- Add cli help design, remove outdate adr
- Mv cli help design
## [0.5.0] - 2026-10-02

### 🚀 Features

- *(search)* Link associated GitHub PRs
- *(index)* Add optional semantic index
- *(index)* Merge optional semantic indexing
- *(search)* Add hybrid semantic retrieval
- *(search)* Merge hybrid semantic retrieval
- *(search)* Chunk long hybrid queries
- *(runtime)* Bundle verified CPU runtime

### 🐛 Bug Fixes

- *(cli)* Remove timeline help callout
- *(search)* Reject code-only flags in text mode
- *(index)* Preserve opt-in and scope help
- *(query)* Scope cache to repository root
- *(semantic)* Reuse vectors across rebuilds
- *(search)* Reject overlong hybrid queries
- *(cache)* Rebuild after history deepening
- *(retrieval)* Filter explicit paths
- *(cache)* Order scans by history position
- *(cache)* Preserve case-sensitive path identity
- *(tests)* Qualify Unix support path
- *(xtask)* Normalize tar member paths
- *(semantic)* Fetch model from repository root
- *(update)* Gate Windows-only io import
- *(ci)* Align native tests with runtime layout
- *(semantic)* Align offline model path

### 🚜 Refactor

- *(github)* Model repository identity
- *(ci)* Simplify package smoke
- *(cache)* Colocate historical scope SQL
- *(cache)* Own semantic vector lifecycle
- *(update)* Unify package staging policy

### 📚 Documentation

- Update Coding agent integration
- Update --add-assignee '@me'
- *(semantic)* Prefer bundled CPU runtime
- *(semantic)* Decouple ORT from vector identity
- *(search)* Clarify GitHub JSON versions
- Keep GitHub notes out of README
- Define current change
- Distinguish historical detail from material
- Set read-only local web scope
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
