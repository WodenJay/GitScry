<div align="center">

<img src="docs/assets/GitScry_logo.png" width="33%" />

# GitScry

*Your Git history is a treasure trove. GitScry uncovers the implementation insights hidden within.*

</div>

## TL;DR

GitScry turns a repository's Git history into actionable context for developers and coding agents.

Instead of manually digging through `git log`, `blame`, and old diffs, GitScry answers questions such as how similar changes were implemented, what approaches failed before, which files and tests tend to change together, why a line exists, and which commits may have introduced a regression.

- **Purpose-built history queries** — 8 commands for common software-engineering questions.
- **More than commit search** — reasons over diffs, reverts, blame, renames, symbols, and co-change history.
- **Traceable results** — findings include the underlying commits, paths, confidence, and supporting signals.
- **Local by design** — GitScry builds a rebuildable cache from your local Git history; Git remains the source of truth.

GitScry is useful when you need to:

- Understand why code exists.
- Find similar past changes.
- Reuse proven implementation patterns.
- Avoid previously failed approaches.
- Discover files that change together.
- Find tests related to your changes.
- Narrow down regression suspects.
- Trace fixes back to introducing changes.

## Installation

```bash
# For Windows
irm xxx

# For Linux/macOS
curl xxx

# For Cargo
cargo xxx
```

Don't want to install manually? Copy this and paste it to your agent:

```text
Read xxx
```

## Usage

```bash
cd your-project

gitscry --help
gitscry index

# Why does this code look this way?
gitscry why src/lib.rs --line 42
```

`gitscry index` builds the local cache from the repository's default-branch history. Run it again whenever you want to refresh the cache.

Want your coding agent to use GitScry? Add this to `AGENTS.md`:

```text
Use GitScry when past changes may help you understand, implement, or debug code. Skip it when history is irrelevant; see `gitscry --help`.
```

## Benchmark

<!-- TODO -->

## Detailed Features

| Command | What it tells you | Example |
|---|---|---|
| **`search`** | General-purpose history search across commit subjects, bodies, and touched paths when no specialized query fits. | `gitscry search retry backoff` |
| **`examples`** | Finds previous implementations of a similar change or migration and reconstructs the files and change steps involved. Changes that were later reverted are demoted rather than presented as good precedents. | `gitscry examples retire provider --path src/lib.rs` |
| **`failures`** | Finds approaches that were abandoned or reverted, together with the recorded reason and retry conditions when history contains them. | `gitscry failures provider normalization` |
| **`related`** | Finds files that historically changed together with the files you are editing. Candidates are ranked from actual co-change history rather than filename similarity. | `gitscry related src/cli.rs src/render.rs` |
| **`tests`** | Finds existing test files that historically changed alongside the code you are modifying, helping surface tests that are easy to overlook. | `gitscry tests src/lib.rs` |
| **`regression`** | Ranks commits that may have introduced an observed regression using affected-path history, symptom matches, diff hunks, symbols, test history, and an optional good→bad revision range. | `gitscry regression slow startup --path src/main.rs --good v0.1.0 --symbol main` |
| **`why`** | Explains the history behind a specific line or symbol by tracing blame, diffs, path history, renames, and explanatory commit messages. | `gitscry why src/lib.rs --symbol provider` |
| **`trace-fix`** | Starts from a known fix and traces deleted/replaced lines back to the change that introduced them, connecting the introducing change, observed failure, and fix. | `gitscry trace-fix HEAD --path src/lib.rs` |

Use `gitscry --help` to learn more.