<div align="center">

<img src="docs/assets/GitScry_logo.png" width="33%" />

# GitScry
<!-- This file can only be edited by humans. Agents do NOT change this file -->
**Give developers and coding agents a memory of how a codebase evolved.**

Find how similar changes were implemented before, why code looks the way it does, which tests tend to move with a file, what approaches failed, and which change may have introduced a bug — directly from local Git history.

</div>

## Contents

- [What GitScry answers](#what-gitscry-answers)
- [Installation](#installation)
- [Quick start](#quick-start)
- [Coding agent integration](#coding-agent-integration)
- [Benchmark](#benchmark)
- [Commands](#commands)
- [Feedback & Issues](#feedback--issues)
- [License](#license)

## See it in action

GitScry turns repository history into answers you can use while implementing, debugging, and reviewing code.

https://github.com/user-attachments/assets/cfbff097-3226-444a-aa37-4859b9586aae

### Find precedent before making a change

```text
$ gitscry examples "cache update" --path src/cache/mod.rs --limit 3

Historical examples (52 matches):
- 5025dd61e230 refactor(cache): centralize memory opening
  step: update src/app/query.rs
  step: update src/cache/mod.rs
  confidence: high

- 278e915d9e08 fix(cache): rehydrate partial history
  step: update src/cache/mod.rs
  step: update src/git/history.rs
  confidence: high

- 82b4fde17dc2 feat(cache): compact cache payloads
  step: update src/analysis/retrieval/history.rs
  step: update src/cache/mod.rs
  step: update src/cache/payload.rs
  ...
  confidence: high
```

### Trace a fix back to the change that introduced it

```text
$ gitscry trace-fix 278e915d9e08 --path src/cache/mod.rs --limit 3

Fix lineage (1 match):
- 8c4e37e2fbab feat(cache): add safe refresh lifecycle
  trace: introducing change at fix 278e915d9e08...
  deleted line: 298
  path: src/cache/mod.rs
  confidence: medium
  commit: 278e915d9e08 fix(cache): rehydrate partial history (fix)
```

### Find tests that historically move with the code you changed

```text
$ gitscry tests src/app/update.rs --limit 3

Historical test candidates (1 match):
- candidate path: tests/cli_contract.rs
  co-change count: 1
  proportion: 100.0%
  supporting commit: 085783099752 feat(update): add self-update command
  confidence: medium
```

These examples are from GitScry's own repository history.

## What GitScry answers

Instead of manually stitching together `git log`, `blame`, old diffs, reverts, and path history, GitScry provides purpose-built queries for questions such as:

- **How was a similar change implemented before?** → `examples`
- **Why does this line or symbol exist?** → `why`
- **Which files historically change together?** → `related`
- **Which tests should I inspect for this change?** → `tests`
- **Was this approach tried and abandoned before?** → `failures`
- **Which commit may have introduced this regression?** → `regression`
- **What change originally introduced the code removed by this fix?** → `trace-fix`
- **Where was this remembered text deleted?** → `trace-removal`
- **What history is relevant to this topic?** → `search`
- **Which existing files were touched repeatedly?** → `hotspots`
- **What direct historical material exists on each side of this text merge conflict?** → `conflicts`

Results stay traceable: GitScry shows the commits, paths, confidence, and evidence behind each finding.

GitScry is local by design. It builds a rebuildable cache from your local repository history; Git remains the source of truth.

## Installation

```bash
# Windows
powershell -ExecutionPolicy Bypass -c "irm https://github.com/WodenJay/GitScry/releases/latest/download/gitscry-installer.ps1 | iex"

# Linux/macOS
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/WodenJay/GitScry/releases/latest/download/gitscry-installer.sh | sh

# Cargo
cargo install gitscry
```

Don't want to install manually? Copy this and paste it to your coding agent:

```text
Read https://github.com/WodenJay/GitScry/blob/main/docs/INSTALL.md to install GitScry.
```

Or download the latest prebuilt binary from [GitHub Releases](https://github.com/WodenJay/GitScry/releases/latest).

## Quick start

```bash
cd your-project

gitscry index

# Find similar past implementations
gitscry examples "retry backoff"

# Understand why code exists
gitscry why src/lib.rs --line 42

# Find tests historically coupled to your change
gitscry tests src/lib.rs
```

`gitscry index` builds the local cache from the repository's default-branch history. Run it again whenever you want to refresh the cache.

## Coding agent integration

Add this to `AGENTS.md`:

```text
Use GitScry when **past changes in the repository** may help with tasks such as understanding, implementing, or debugging code. Skip it when past changes are irrelevant. Run `gitscry --help` for more information.
```

This is enough! GitScry intentionally uses a single lightweight instruction instead of an MCP server, a dedicated skill, or a full command list.

- Not every session needs `gitscry`, so keeping GitScry permanently exposed through MCP would consume context even when it is irrelevant. 
- A dedicated skill would duplicate information that is already available through `gitscry --help`. 
- Listing every command in the prompt would have the same problem as a dedicated skill: it increases context usage before the agent even knows whether GitScry is needed.

## Benchmark

| Command | Repository | Test command | Result |
|---|---|---|---|
| `search` | `NousResearch/hermes-agent` | `gitscry search "stealth ox-alpha model catalog" --limit 5` | Both relevant prior changes ranked in the Top 5 (ranks 3 and 4). |
| `examples` | `curl/curl` | `gitscry examples "digest quote the digest-uri param as well" --limit 5` | Matching precedent ranked #2; shared source file cited. |
| `tests` | `NousResearch/hermes-agent` | `gitscry tests agent/reasoning_timeouts.py --limit 5` | Relevant test file ranked #1. |
| `related` | `git/git` | `gitscry related builtin/gc.c --limit 5` | Related test file ranked #1. |
| `regression` | `git/git` | `gitscry regression "worktree add dwim when b or B is given" --path builtin/worktree.c --good <good-revision> --bad <bad-revision> --limit 5` | Relevant change ranked #2. |
| `why` | `git/git` | `gitscry why builtin/name-rev.c --line 99 --at <historical-revision> --limit 5` | Correct explanation ranked #1. |
| `trace-fix` | `rust-lang/rust` | `gitscry trace-fix <fix-commit> --path library/stdarch/crates/core_arch/src/x86/avx2.rs --limit 5` | Introducing change ranked #2. |

## Commands

Historical queries use current HEAD's reachable Git history intersected with the published cache. If reachable history is not fully cached, available commits are used and incomplete coverage is reported with `gitscry index` guidance. Explicit revision bounds must be cached; use `--to-rev` to investigate another cached branch.

| Command | What it tells you | Example |
|---|---|---|
| **`search`** | General-purpose history search across commit subjects, bodies, and touched paths; also supports literal `--code` and regex `--code-regex` searches over changed lines. | `gitscry search --code-regex 'Old[A-Z][A-Za-z0-9_]*' --change removed --path src/lib.rs` |
| **`examples`** | Finds previous implementations of a similar change or migration and reconstructs the files and change steps involved. Changes that were later reverted are demoted rather than presented as good precedents. | `gitscry examples retire provider --path src/lib.rs` |
| **`failures`** | Finds approaches that were abandoned or reverted, together with the recorded reason and retry conditions when history contains them. | `gitscry failures provider normalization` |
| **`related`** | Finds files that historically changed together with the files you are editing. Candidates are ranked from actual co-change history rather than filename similarity. Add `--patterns` for closed file-incarnation combinations supported by commits containing every seed (default minimum support 3; `--min-support 2` allows pairs of supporting commits). Groups contain at least three incarnations, include exact support/proportions and capped path-specific citations, and retain deleted history. Detected renames connect paths; deletion ends an incarnation, while recreation and copies start new ones. Pattern mode is local-only. | `gitscry related src/cli.rs src/render.rs` |
| **`tests`** | Finds existing test files that historically changed alongside the code you are modifying. | `gitscry tests src/lib.rs` |
| **`regression`** | Ranks commits that may have introduced an observed regression using affected-path history, symptom matches, diff hunks, symbols, test history, and an optional good→bad revision range. | `gitscry regression slow startup --path src/main.rs --good v0.1.0 --symbol main` |
| **`why`** | Explains the history behind a specific line or symbol by tracing blame, diffs, path history, renames, and explanatory commit messages. | `gitscry why src/lib.rs --symbol provider` |
| **`trace-fix`** | Starts from a known fix and traces deleted/replaced lines back to the change that introduced them. | `gitscry trace-fix HEAD --path src/lib.rs` |
| **`trace-removal`** | Discovers literal deleted-text events grouped by commit and historical old path, with first-parent locators and complete commit messages. Limits events, not matching lines; does not infer retirement or replacement. | `gitscry trace-removal --code 'legacy()' --path src/lib.rs --json` |
| **`hotspots`** | Ranks tracked files at current HEAD using distinct non-merge touching commits cached and reachable from HEAD; incomplete cached coverage is reported. Adds cached textual additions/deletions from those touches. JSON additions/deletions are null when no eligible diff is calculable; `churn_complete` is false if any eligible diff is unavailable. Human output marks partial counts with * and unavailable counts with —. | `gitscry hotspots --json --limit 20` |
| **`index`** | Builds or refreshes the local cache from the repository's default-branch history. | `gitscry index` |
| **`update`** | Updates GitScry to the latest stable release. | `gitscry update` |
<!-- This file can only be edited by humans. Agents do NOT change this file -->
Use `gitscry --help` to learn more.

## Feedback & Issues

GitScry is still evolving, and real-world Git histories can contain many edge cases.

If you encounter incorrect results, unsupported scenarios, or have ideas for improvement, please feel free to [open an issue](https://github.com/WodenJay/GitScry/issues). Bug reports with a reproducible repository or commit example are especially helpful.

## License

GitScry is available under the [MIT License](LICENSE).

[GitHub repository](https://github.com/WodenJay/GitScry) · [crates.io package](https://crates.io/crates/gitscry)
