# GitScry Dogfooding

Whenever GitScry is used, briefly assess whether it meaningfully helped with the intended task.

- Functionality **already available** in the installed GitScry version -> use `gitscry`
- Functionality **currently under development** -> use `cargo run`, because the feature is not in installed `gitscry` yet.

Record a case only when it did not—for example, its output was not useful, it failed to provide needed information, or its behavior materially differed from what was reasonably expected. Do not record merely imperfect uses or create entries just for the sake of dogfooding.

## Template

### [Date] Short description

- **Command:** `gitscry ...`
- **Context:** GitScry version and invocation HEAD
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

### 2026-10-06 Failure count diagnosis

- **Command:** `cargo run --quiet -- failures report count --path src/analysis/capabilities/failures.rs --limit 5 --json`; `cargo run --quiet -- failures sandbox --limit 100 --json`
- **Context:** Development build of GitScry 0.12.0 at HEAD `38ae025` on `fix/issue-242`.
- **Goal:** Find relevant failure-count history and reproduce issue #242 using GitScry on this repository.
- **Actual:** Both queries returned `matched_count=0` with no materials; this repository offered no relevant history or candidates, so dogfooding did not identify prior implementation guidance or reproduce the issue. A controlled test-repository CLI fixture was needed.
- **Expected:** Failure history or matching candidates that expose the mismatch between retrieval candidates and eligible failed approaches.
