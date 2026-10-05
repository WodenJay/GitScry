# GitScry Dogfooding

Whenever GitScry is used, briefly assess whether it meaningfully helped with the intended task.

Record a case only when it did not—for example, its output was not useful, it failed to provide needed information, or its behavior materially differed from what was reasonably expected. Do not record merely imperfect uses or create entries just for the sake of dogfooding.

## Template

### [Date] Short description

- **Command:** `gitscry ...`
- **Context:** GitScry version and invocation HEAD; relevant working-tree state if the query depends on it.
- **Goal:** What I wanted to learn or accomplish.
- **Actual:** What GitScry returned or failed to do. Include the key output and relevant scope, coverage, or warnings; omit unrelated output.
- **Expected:** What would have been useful instead.

<!-- DO NOT EDIT ABOVE THIS LINE. APPEND NEW ENTRIES BELOW. -->

## Entries


### 2026-10-05 Target-scoped tests query

- **Command:** `gitscry tests src/seed.rs --line 1 --json`
- **Context:** Installed GitScry 0.11.0; invocation HEAD `9920f49` (`main`, clean). The worktree feature binary was not used.
- **Goal:** Retrieve current test-path associations for a selected source line while implementing #225.
- **Actual:** Failed before querying: `error: unexpected argument '--line' found`; the CLI suggested `--since`. No history material was returned.
- **Expected:** Target-touch scoped associations with target identity, eligible-touch denominator, and current test candidates.


### 2026-10-05 Target history at merge boundary

- **Command:** `cargo run --quiet -- tests src/analysis/capabilities/relations.rs --line 61 --json`
- **Context:** Worktree build `gitscry 0.11.0`, invocation HEAD `9920f49` (`feature/tests-target-scope-225`); six modified files. The query target revision was HEAD.
- **Goal:** Dogfood target-scoped test associations on the implementation's `execute_target` path.
- **Actual:** Returned no materials, `status: "unavailable"`, `eligible_target_touch_commits: null`; limitation: `Tracing stopped at a merge or shallow-history boundary; earlier target touches are indeterminate.`
- **Expected:** Available target-touch history and current test candidates for the selected line.
