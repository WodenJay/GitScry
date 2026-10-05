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

### 2026-10-05 Directory move-out exclusion

- **Command:** `cargo run --quiet -- related src/analysis/retrieval/ --json --limit 5`
- **Context:** Development build at HEAD `aae1b41` on `feat/related-directory-follow-on`, with uncommitted candidate-exclusion changes.
- **Goal:** Check whether a repository-level directory query would expose moved-out source paths as candidates.
- **Actual:** The query refreshed history and returned 85 matches; its leading co-change candidates included `src/analysis/mod.rs`. It completed successfully but provided no specific moved-out-source evidence, so it did not validate the exclusion bug.
- **Expected:** A controlled move-out history or surfaced candidate/path attribution that distinguishes the moved source incarnation from independent candidates.
