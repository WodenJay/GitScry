## [0.14.0] - 2026-10-08

### 🚀 Features

- *(context)* Prioritize tests tied to matched changed-code history
- Aggregate followups diagnostics with --verbose override
- Add propagation multi-target exact reachability
- Bind reverts to explicit declarations only
- Add repeatable --ref option to index command
- *(search)* Add repeatable --path changed-path scope to QUERY and hybrid search
- *(fate)* Add forward line fate tracking (WIP wiring)
- *(search)* Find strict patch equivalents
- *(search)* Merge strict patch equivalents
- *(propagation)* Detect whole-commit patch equivalents
- *(fate)* Follow historical symbol edits
- *(fate)* Follow strict target moves
- *(search)* Find inverse patch matches
- *(examples)* Group patch-equivalent precedents
- *(trace-fix)* Show equivalent fix versions
- *(fate)* Track merge ancestry
- *(patch-search)* Search current changes
- *(followups)* Surface patch relationships
- *(failures)* Add inverse patch leads
- *(failures)* Merge inverse patch leads

### 🐛 Bug Fixes

- *(context)* Merge matched tests before followup passes, dedupe warning
- Pluralize followups diagnostic summaries and index-based warning stripping
- Keep propagation answers exact and degrade to indeterminate, not error
- Revert declarations start sentences and share one target resolver
- *(fate)* Continue tracking past 1:1 rewrites and raise Windows stack reserve
- *(fate)* Honor line continuity rules
- *(search)* Disclose patch operation provenance
- *(search)* Verify cached patch availability
- *(render)* Escape patch provenance safely
- *(propagation)* Preserve patch matches with gaps
- *(fate)* Require preserved symbol correspondence
- *(propagation)* Classify special patch gaps
- *(propagation)* Report patch gaps
- *(fate)* Require added move candidates
- *(fate)* Retain uncertain continuations
- *(examples)* Use dual patch fingerprints

### 🚜 Refactor

- *(search)* Harden --path validation and clean up scope plumbing
- *(git)* Name complete patch data
- *(trace-fix)* Own version report
- *(analysis)* Localize failure lead data

### 📚 Documentation

- Remove redundent content in README
- Update index content in README
- Log inverse patch dogfood result
- Shrink DOGFOODING rule, remove cargo run manner
- Remove non-actionable dogfood entry
- Record followups dogfood gap
- Log followups patch match dogfood
- Drop invalid cargo-run dogfood logs
- Record failure-query dogfooding
## [0.13.0] - 2026-10-06

### 🚀 Features

- *(conflicts)* Add safe historical cases
- *(symbol)* Resolve structured Rust selectors
- *(symbol)* Expose pinned selection metadata
- *(symbol)* Share traced history and notices
- *(symbol)* Merge Rust structured history
- *(symbol)* Resolve structured Python selectors
- *(symbol)* Resolve structured Go selectors
- *(conflicts)* Report association basis and region trace for historical cases
- *(conflicts)* Rank and expose inspectable cases
- *(conflicts)* Disclose historical case coverage
- *(symbol)* Add structured JS/TS symbol extraction
- *(conflicts)* Bound historical checks explicitly
- *(symbols)* Structured C++ symbol extraction
- Dual-grammar interpretation for C/C++ header symbols

### 🐛 Bug Fixes

- *(regression)* Require unique symbol targets
- *(symbol)* Scan every declaration candidate
- *(context)* Preserve follow-up ranking
- *(context)* Separate identity from scope
- *(context)* Merge identity scope fix
- *(failures)* Count eligible results
- *(symbol)* Disclose degradation in text
- *(symbol)* Guard historical ownership and copies
- *(conflicts)* Correct candidate coverage accounting and bump schema to 6
- *(symbol)* Keep TS method decorators and export decorations in spans
- *(tests)* Pin the noVNC fixture to LF line endings
- *(test)* Point c query helper at util.c
- Preserve symbol scope disclosure precedence

### ⚡ Performance

- *(cache)* Retain decoded hunk blocks
- *(context)* Batch current path modes
- *(history)* Share query graph preparation
- *(test)* Batch fixture identity and fillers
- *(conflicts)* Reuse targeted tree metadata
- Merge test runtime improvements

### 🚜 Refactor

- *(symbol)* Retain declaration identity
- *(history)* Remove redundant borrows
- Share symbol history confirmation
- Centralize conflict candidate counters

### 📚 Documentation

- Record hotspot dogfooding timeout
- Change block type from bash to console in README
- Define patch relationship boundaries
## [0.12.0] - 2026-10-05

### 🚀 Features

- *(search)* Add exact code fragment lookup
- *(related)* Add directory co-change
- *(related)* Merge directory co-change
- *(related)* Scope history to selected line
- *(related)* Merge line-scoped history
- *(relations)* Support mixed sources
- *(relations)* Merge mixed-source support
- *(trace-removal)* Trace deleted fragments
- *(related)* Add symbol targeting
- *(related)* Merge symbol targeting
- *(tests)* Scope associations to targets
- *(related)* Add directory follow-on
- *(related)* Merge directory follow-on

### 🐛 Bug Fixes

- *(regression)* Respect file incarnations
- *(regression)* Disclose unresolved identity
- *(regression)* Merge incarnation boundary fix
- *(search)* Clarify fragment limits and coverage
- *(related)* Align source resolution diagnostics
- *(related)* Preserve degraded line history
- *(related)* Exclude moved source candidates

### 📚 Documentation

- Add cli rule about simple and direct
- Add CODING_STANDARDS.md
- *(dogfooding)* Distill code fragment search to issue
- *(cli)* Define concise help rules
- Remove wrong dogfooding
- Refine cargo fmt rule in AGENTS.md
- Refine usage of gitscry in DOGFOODING.md
- *(cli)* Trim help into a usage manual
- Replace command to real --help output in README
## [0.11.0] - 2026-10-05

### 🚀 Features

- *(context)* Rank historical follow-ups
- *(context)* Add follow-up check budget
- *(render)* Bound query stdout, persist overflow
- *(render)* Merge bounded query output
- *(related)* Add directional follow-on material
- *(related)* Merge directional follow-on material
- *(context)* Add shared path follow-on material

### 🐛 Bug Fixes

- *(release)* Strip blank checksum lines
- *(context)* Merge budget with auto refresh
- *(render)* Restrict Windows spill file access
- *(related)* Exclude ambiguous boundary events
- *(context)* Clarify follow-on counts and controls
- *(release)* Normalize checksums before announce

### 📚 Documentation

- Record context dogfooding
- Add cli rule for assertions about output
- Centralize test contract rules
- Protect README
- *(related)* Explain follow-on material
- Capture dogfooding runtime context
## [0.10.0] - 2026-10-04

### 🚀 Features

- *(context)* Detect historical follow-ups
- *(context)* Merge historical follow-ups
- *(query)* Refresh initialized HEAD history
- *(cache)* Refresh semantic coverage on queries
- *(query)* Unify historical refresh

### 🐛 Bug Fixes

- *(context)* Respect current file identity
- *(help)* Match shared query refresh behavior
- *(regression)* Exclude preceding deletions
- *(paths)* Canonicalize historical filters
- *(conflicts)* Use shared cache refresh

### 🚜 Refactor

- *(query)* Own target preparation protocol
- *(cache)* Encapsulate incarnation queries

### 📚 Documentation

- Refine DOGFOODING
- *(adr)* Allow automatic cache refresh
- Record follow-up dogfood gap
- *(query)* Explain automatic history refresh
- Remove redundant --json help content
## [0.9.0] - 2026-10-04

### 🚀 Features

- *(conflicts)* Add two-side cached leads
- *(conflicts)* Add conflict leads
- *(conflicts)* Trace leads across renames
- *(conflicts)* Surface associated material
- *(conflicts)* Include shared history
- *(conflicts)* Integrate shared history
- *(conflicts)* Trace file-level histories

### 🐛 Bug Fixes

- *(git)* Share declaration recognition
- *(context)* Preserve sparse index flags
- *(conflicts)* Disclose evidence limits
- *(cache)* Restore pruned commits
- *(conflicts)* Escape output, scope truncation
- *(conflicts)* Prioritize reverted history

### 📚 Documentation

- Clean README
- Add gitscry DOGFOODING.md
- Update DOGFOODING rule
- Define material-first agent boundary
- Prohibit LLM integration
- Keep cache updates out of queries
- Make query execution budgets opt-in
- Refine DOGFOODING rule
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
- *(test)* Import Unix shared command helper
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
