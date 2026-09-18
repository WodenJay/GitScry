# Validation repositories for the 11 historical-capability prototypes

Status: research note (first-hand data only). Measured 2026-09-18 from this Windows host via an
authenticated `gh` CLI and local `git` clones. No benchmark was run and no test suites were
executed — every number below comes from the repository's own history/tree or the GitHub REST API.

## Identity of the two candidates

| | Primary candidate | Review candidate |
|---|---|---|
| Repo | [NousResearch/hermes-agent](https://github.com/NousResearch/hermes-agent) | [rust-lang/rust](https://github.com/rust-lang/rust) |
| Created / default branch | 2025-07-22 / `main` | 2010-06-16 / `main` |
| Language | Python ~74%, TypeScript ~25% (REST `/languages`) | Rust (+ C++ via `src/llvm-project`) |
| License | MIT | Apache-2.0 / MIT |

The user's "hermes-agent" is confirmed as `NousResearch/hermes-agent` ("The agent that grows with
you"), not a fork or the many community derivatives the GitHub search surfaces (3,750 name
matches; the official repo has 246k stars). Source: <https://github.com/NousResearch/hermes-agent>.

## Cost of cloning and analysing

| Metric | hermes-agent | rust-lang/rust | How measured |
|---|---|---|---|
| Commits on `main` | 36,743 | 340,409 | `git rev-list --count HEAD` |
| History span | 422 days (~87 commits/day) | 5,937 days (~57/day) | first vs. last `%cI` |
| Files at HEAD | 13,894 | 62,918 | `git ls-tree -r HEAD` |
| Full bare clone | 981 MiB / 4m18s (`.git` 779 MiB) | 1.1 GiB / 4m06s | `git clone --bare --single-branch` |
| `--filter=blob:none` bare | **81 MiB / 24s** | 434 MiB / 2m37s | tree+commit objects, blobs on demand |
| `--filter=tree:0` bare | **37 MiB / 13s** | 140 MiB / 1m26s | commit objects only |
| `--shallow-since=2025-01-01` bare | n/a (all history is in range) | 281 MiB / 58,102 commits / 2m23s | truncated-history middle ground |
| Full-history metadata walk | 1.0s (79 MiB text) | 5.2s (92 MiB text) | `git log --format=...` |
| `git log -100000 --name-only` | n/a (fewer commits) | 8.4s on a **full** clone | see caveat |

Caveats that matter for GitScry:

- **Partial clones make history walks network-bound.** On the `--filter=blob:none` rust clone,
  `git log -20000 --name-only` spent 2m16s and then failed on an unreachable pack. With a full
  clone the same command takes 1.7s. `--filter=tree:0` omits the trees that path/diff derivation
  needs, so it only buys cheap commit-metadata indexing.
- rust's tree is served truncated by the Git Trees API (64,726 entries returned, `truncated: true`),
  so any API-only analysis undercounts; blob-path counts below are therefore lower bounds.
- rust head composition: 60,072 blobs / 223 MB of blob content, dominated by `.rs` 37,216,
  `.stderr` 14,844, `.md` 1,482, `.fixed` 1,362, `.diff` 920. `tests/` alone is 41,398 files,
  `tests/ui` 33,067 (19,709 `.rs` + 14,844 `.stderr`).
- rust has 20+ shallow submodules including `src/llvm-project` and `src/tools/cargo`
  ([`.gitmodules`](https://github.com/rust-lang/rust/blob/main/.gitmodules)); blame/why analysis
  must stop at submodule boundaries.

## Commit-message and workflow quality

| Signal | hermes-agent | rust-lang/rust |
|---|---|---|
| Conventional-commit subjects | 32,622 / 36,743 (**89%**), documented in [`CONTRIBUTING.md`](https://github.com/NousResearch/hermes-agent/blob/main/CONTRIBUTING.md) | not used; title + free-form body |
| Bot-generated subjects | negligible | 79,321 / 340,439 (**23%**) are `Auto merge of #N …` / `Rollup merge of #N …` |
| `fix` subjects | 18,631 | 22,386 non-merge, 70,114 via search |
| Merge commits | 2,896 / 36,743 (7.9%); last 1,000 commits → **3** | 107,538 / 340,409 (31.6%); last 1,000 → 8,339 |
| Median subject length | 69 chars | 49 chars |
| Inline issue/PR refs in messages | 7,856 subjects contain `#NNNN`; 9,100 "fixes #"; 5,764 "fix #" | 23,503 "fixes/closes #" overall, 6,551 in non-merge bodies |
| Commit bodies | long, why-focused, present on most commits | non-merge: short-to-medium; **PR description lands on the merge commit** (verified on `Auto merge of #162573`) |
| Revert evidence | 555 revert-ish subjects, **338** "This reverts commit", 60 `Revert "…"` | 1,593 `Revert` subjects, **1,388** "This reverts commit" |
| Formatting/lockfile noise | 1,190 `chore:` (release/fmt/contributors), `scripts/release.py` in 1,348 commits | ~2,645 rustfmt/"format the world" commits; `Cargo.lock` in 72/500 merges |
| Blame hygiene | no `.git-blame-ignore-revs` (404) | [`.git-blame-ignore-revs`](https://github.com/rust-lang/rust/blob/main/.git-blame-ignore-revs) present |
| History anomalies | [history-check.yml](https://github.com/NousResearch/hermes-agent/blob/main/.github/workflows/history-check.yml) documents PR #25045 re-rooting blame for ~1,500 files | n/a |

Representative hermes commit (why is written down, issue references are inline, tests are named):

```
fix(desktop): single-owner console capture + HUD lifecycle coverage

Reconcile the salvaged #81533 lifecycle helper with the renderer-log
console pipeline that landed in #83535 (the two PRs raced):
…
- Tests updated: lifecycle helper asserts it attaches NO console-message
  listener; parser tests live in renderer-log.test.ts.
```

Source: <https://github.com/NousResearch/hermes-agent/commit/0a60b164f56e632544600bf368e374c0ef4e8986>.
Repositories: hermes 28,940 issues / 85,639 PRs / 14,323 merged; rust 63,908 issues / 97,883 PRs /
74,044 merged.

## Test structure

| | hermes-agent | rust-lang/rust |
|---|---|---|
| Layout | `tests/<module>/` mirrors source modules; 4,555 `.py` test files, 43,864 `def test_` | compiletest fixtures under `tests/ui/`, keyed to diagnostics, not modules |
| Runner | `scripts/run_tests.sh` (per-file subprocess isolation), `pyproject.toml` `testpaths=["tests"]` | `./x test`; `tests/ui` `.rs` + `.stderr` pairs |
| Source↔test correspondence | strong: `gateway/` 114 src files ↔ 884 test files, `agent/` 231 ↔ 807, `hermes_cli/` 378 ↔ 1,176, `cron/` 28 ↔ 128 | weak/indirect: `compiler/` 2,182 `.rs` ↔ `tests/ui` 19,709 `.rs` fixtures |
| Fix commits that also touch tests | 1,363 / 1,823 (**75%**) of the last 3,000 | 832 / 2,999 non-merge (28%) of a 3,000 sample; 322/500 first-parent merges (64%) touch both code and tests |
| Issue-tagged regression fixtures | tests referencing regressions exist but no path convention | **7,163 paths contain `issue-NNNN`** (5,840 under `tests/ui/`, 6,270 under `tests/`); code search: 1,456 `tests/ui` files mention "issue #" |
| CI | 36 workflows incl. `tests.yml`, `js-tests.yml`, `e2e-desktop.yml` | `rust-bors`, tidy, compiletest gates |

## Per-capability coverage

`● primary-material / ◐ usable / ○ weak` on the chosen role.

| Capability | hermes-agent (primary) | rust-lang/rust (review) | Evidence |
|---|---|---|---|
| 1. Historical semantic search | ● 36.7k commits, 89% conventional, long subjects/bodies | ◐ 340k commits but 23% bot merge subjects | subject-quality table above |
| 2. Code why | ● why-first bodies + inline PR refs | ◐ PR description is on the `Auto merge` commit; landed commit → merge mapping needed | verified `Auto merge of #162573` body = PR description; `git rev-list --ancestry-path --merges` recovers it |
| 3. Co-change related | ● i18n family co-changes 12–15×/1,000 commits; 459/1,000 commits touch code+tests | ◐ sparser: `library/alloc/src/rc.rs`↔`sync.rs` 19×/3,000; `rustc_ast/mut_visit.rs`↔`visit.rs` 18×/3,000 | name-only scans |
| 4. Missed-change detection | ● mirrored-file template: `i18n/en.ts` touched 41×, 5× without `zh.ts`; `hermes_state_*` family | ◐ `*.rs`↔`*.stderr` fixture pairs (493/3,000 commits) + `rustc_span/src/symbol.rs` listings | last-1,000/3,000 scans |
| 5. Test impact mapping | ● per-module `tests/<x>` mirroring, 75% of fixes touch tests | ○ compiletest is diagnostic-keyed; mapping is batch/coarse | table above |
| 6. Historical modification examples | ● 3.63 files/commit, 3,000-commit samples are small and focused | ◐ merges average 65.5 files (max 1,105); individual commits are small but land in batches | first-parent merge scan |
| 7. Regression localization | ◐ real regression tests, weak path convention | ● `issue-NNNN` fixture corpus, explicit regression-test convention | 7,163 issue-tagged paths |
| 8. SZZ bug lineage | ● fix subjects + `fixes #` inline in the same commit; squash-linear | ◐ rich but issue refs split between merged commit and PR body; 31.6% merge noise | message scans |
| 9. Revert / failed memory | ● 555 revert-ish, 338 "This reverts commit", semantic feature rollbacks | ● 1,593 `Revert` subjects, 1,388 "This reverts commit" | message scans |
| 10. Hotspot / risk | ◐ clear churn (`gateway/run.py` 1,857 touches, `cli.py` 1,208, `run_agent.py` 1,290) but only 14 months of history | ● 16 years; `symbol.rs`, `bootstrap/*`, `rustc_span` hotspots | name-only frequency |
| 11. Change clusters | ● tight single-language clusters (`apps`, `hermes_cli`, `agent`, `gateway`, `tools`) | ◐ clusters span compiler/library/tools/submodules, merge-batched | last-1,000 commit → top-level dir counts, first-parent sample |

## Recommendation

**Use hermes-agent as the primary validation repository; use rust-lang/rust as a lightweight,
bounded review/stress repository — not a second full workload.**

Why hermes-agent is primary:

- **Cheapest full-fidelity index.** `--filter=blob:none` bare clone is 81 MiB / 24s and keeps trees,
  so every capability except raw-blob grepping can run offline; full-history metadata walks take ~1s.
  No submodules, no partial-clone network dependency.
- **Highest signal per commit for exactly the capabilities we prototype.** 89% conventional subjects,
  why-focused bodies, issue numbers inlined in the same commit, 75% of fixes touching tests, and a
  per-module `tests/<x>` mirror that makes test-impact and missed-change detection testable on real
  cases (the `i18n/en.ts` without `zh.ts` misses are concrete ground truth).
- **Single-language, single-app clusters** (Python + one TS desktop app) keep co-change and
- **Cohesive application clusters** across Python services and one TypeScript desktop app keep co-change
  and cluster prototypes understandable without rust's compiler/library/toolchain/submodule confounds.
- **Real revert/failed-approach material** with stated reasons, useful for capability 9.

Why rust-lang/rust is review-only:

- It is the right **differential/stress** case — non-conventional messages, 23% bot merge commits,
  batched 65-file merges, a 7,163-file `issue-NNNN` regression corpus, 16-year blame, submodule
  boundaries, and a `.git-blame-ignore-revs` file. These are exactly the conditions that break naive
  implementations.
- But it is **expensive and awkward by default**: 1.1 GiB / 4m06s full clone, and after a
  `--filter=blob:none` clone any `--name-only`/diff walk becomes network-bound (observed 2m16s then
  failure). A full 37k-file compiletest corpus cannot be exercised offline cheaply.
- Its commit→PR mapping is non-trivial: the "why" lives on the `Auto merge of #N` merge commit, so
  landed-commit analysis must walk ancestry to the merge (verified with `git rev-list
  --ancestry-path --merges`).

### Suggested lightweight protocol

1. **Primary loop (hermes-agent, offline):**
   ```bash
   git clone --bare --filter=blob:none --single-branch --branch main \
     https://github.com/NousResearch/hermes-agent.git hermes-agent.git
   ```
   Index `git log --format='%H%x01%an%x01%aI%x01%s%x01%b'` + `--name-only`; validate all 11
   capabilities against the concrete cases in the table above.

2. **Review pass (rust-lang/rust, bounded):** only for scale claims and adversarial conditions. Use a
   **full** bare clone when running path/diff walks (partial clones are slower here), or
   `--shallow-since=2025-01-01` (281 MiB, 58,102 commits) when 16-year blame is not under test, and
   cap every walk to a window (e.g. `git log -N --first-parent`) and to `compiler/` + `library/` +
   `tests/ui/`.

3. **Known blind spots to state explicitly in prototype results:** hermes has only ~14 months of
   history, so hotspot/risk *trends* (capability 10) and long-horizon SZZ lineage are thin, and it
   has no `.git-blame-ignore-revs` — rust should be used to confirm those two behaviours. rust's
   merge-batched diffs must be handled by first-parent or ancestry mapping, not raw commit diffs.

## Primary sources

- Repos: <https://github.com/NousResearch/hermes-agent>, <https://github.com/rust-lang/rust>
- REST metadata: `GET /repos/{owner}/{repo}`, `/commits?per_page=1` (Link `rel="last"`),
  `/contributors`, `/languages`, `/git/trees/main?recursive=1`, `/contents/…`
- hermes: [CONTRIBUTING.md](https://github.com/NousResearch/hermes-agent/blob/main/CONTRIBUTING.md),
  [AGENTS.md](https://github.com/NousResearch/hermes-agent/blob/main/AGENTS.md),
  [history-check.yml](https://github.com/NousResearch/hermes-agent/blob/main/.github/workflows/history-check.yml),
  [run_tests.sh](https://github.com/NousResearch/hermes-agent/blob/main/scripts/run_tests.sh),
  [PR #114666](https://github.com/NousResearch/hermes-agent/pull/114666)
- rust: [.gitmodules](https://github.com/rust-lang/rust/blob/main/.gitmodules),
  [.git-blame-ignore-revs](https://github.com/rust-lang/rust/blob/main/.git-blame-ignore-revs),
  [tests/ui](https://github.com/rust-lang/rust/tree/main/tests/ui),
  [CONTRIBUTING.md](https://github.com/rust-lang/rust/blob/main/CONTRIBUTING.md)
- Local measurements were taken on clones under `/tmp/gs` (hermes-full, hermes-nb, hermes-bare,
  rust-full, rust-nb, rust-bare, rust-shallow); they are scratch artifacts and are not part of
  GitScry.
