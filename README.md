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
- **What direct historical material exists on each side of this merge conflict?** → `conflicts`

Results stay traceable: GitScry shows the commits, paths, confidence, and evidence behind each finding.

GitScry is local by design. It builds a rebuildable cache from your local repository history; Git remains the source of truth.

### Cache initialization and refresh

Run `gitscry index` once per repository to initialize its shared cache. Search and capabilities using shared query preparation then synchronously add missing locally available history reachable from the invocation's pinned HEAD, including merged parents, before selecting material. Previously cached branch history is retained; query filters do not limit maintenance. Covered targets remain quiet even when the published cache tip differs.

Queries do not initialize, repair, upgrade, fetch, or scan all refs. Refresh and lock waits report progress on stderr, without a default timeout. Shallow and missing-object limitations remain disclosed. A failed refresh uses prior published material only when the cache is safely readable and query prerequisites hold, with explicit failure and incomplete-coverage warnings in human and JSON reports. Semantic settings remain unchanged; hybrid queries still require compatible semantic coverage. Explicit `index` remains available for maintenance and semantic preparation.

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

```bash
$ gitscry --help

Usage: gitscry <COMMAND>

Query commands:
  conflicts      Find historical file changes for each side of a merge conflict
  followups      Find later reverts and changes to a commit's files or added lines
  hotspots       Rank files by commit count and added/deleted lines
  context        Find historical code matches, abandoned changes, related files and tests
  trace-removal  Find commits that deleted matching code
  search         Search commit messages, paths, or changed code
  examples       Find past implementations of a similar change or migration
  failures       Find reverted or abandoned approaches and their recorded reasons
  related        Find files changed with or shortly after seed files or directories
  tests          Find current tests changed alongside files, a line, or a symbol
  regression     Find commits that may have introduced a regression
  why            Show blame attribution and changes to a line or symbol
  trace-fix      Trace a fix to earlier changes and its recorded failure
  timeline       Show a file's history from earliest to latest

Management commands:
  stats          Show local command counts and elapsed time
  update         Update GitScry to the latest stable release
  index          Build or refresh the repository history cache
  clear          Delete the repository's GitScry cache and settings
  prune          Remove cached commits whose Git objects no longer exist locally
  help           Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
Run `gitscry <command> --help` for arguments, options, and examples.
```

## Feedback & Issues

GitScry is still evolving, and real-world Git histories can contain many edge cases.

If you encounter incorrect results, unsupported scenarios, or have ideas for improvement, please feel free to [open an issue](https://github.com/WodenJay/GitScry/issues). Bug reports with a reproducible repository or commit example are especially helpful.

## License

GitScry is available under the [MIT License](LICENSE).

[GitHub repository](https://github.com/WodenJay/GitScry) · [crates.io package](https://crates.io/crates/gitscry)
