# Issue 4: change-relation capability replay

Status: prototype result for human review. The code and numbers here are disposable, not a production design.

## Verdicts

| Capability | Verdict | Reason |
|---|---|---|
| `related` | **keep** | Repeated co-change compresses many path-history walks into concise, commit-traceable relations, including cross-tree source/test links. |
| `check-change` | **defer** | The same strong relation can identify a hidden historical companion or falsely accuse an intentional partial change; history alone cannot tell which. |
| `tests` | **keep** | Filtering co-change relations to tests found specific suites beyond a same-name baseline, while exposing support rather than claiming certainty. |
| `risk` | **drop** | Touch frequency mostly identified central registries, translations, and rollup churn; it missed novel code and added no information beyond `git log -- <path>`. |
| `clusters` | **drop** | The strongest groups restated visible source/test and package layout at lower resolution than `related`. |

`keep` means the replay produced useful `material`, not that the final MVP must expose a separate command; the MVP-selection ticket can still combine `related` and `tests`.

## Protocol

The primary clone contained all 36,770 commits reachable from `NousResearch/hermes-agent` `main`. Each case log contained the held-out commit followed only by commits reachable from its first parent, preventing future or sibling-branch leakage. Merge changes were compared with their first parent. Commits changing more than 50 paths were excluded from pair counts so mass formatting and merges did not manufacture relations.

For a seed path and candidate path, the probe reports:

- `n`: older commits changing both paths;
- `p`: `n / older commits touching the seed`;
- rank score: `n * p`, a deliberately simple support discount.

Baselines were a sibling/name inspection (`ls-tree`, mirrored `tests/` paths) and `git log -- <path>`. No benchmark or test suite was built or run. [`replay.py`](replay.py) is the complete probe; it uses only Python's standard library and has a `self-check` command.

## `related`: keep

### Locale family: strong but mostly visible from paths

Before [`27dfbe3`](https://github.com/NousResearch/hermes-agent/commit/27dfbe397f849116c116e82e189499f747f62032), `apps/desktop/src/i18n/en.ts` had 525 eligible prior touches. Its top relations were:

| Candidate | `n` | `p` | In held-out change |
|---|---:|---:|---|
| `i18n/zh.ts` | 505 | 96.2% | yes |
| `i18n/types.ts` | 456 | 86.9% | yes |
| `i18n/zh-hant.ts` | 390 | 74.3% | yes |
| `i18n/ja.ts` | 389 | 74.1% | yes |
| `i18n/ar.ts` | 193 | 36.8% | yes |
| `i18n/ru.ts` | 49 | 9.3% | yes |

All six relations replayed correctly, but `git ls-tree ... apps/desktop/src/i18n` already exposes the family. History contributes strength and traceable commits, not discovery by itself.

### Honcho CLI: useful cross-tree relation

For [`476dbfe`](https://github.com/NousResearch/hermes-agent/commit/476dbfed3a76b85985674dc30e484de79859955f), `plugins/memory/honcho/cli.py` historically related most strongly to `tests/honcho_plugin/test_cli.py` (`n=29`, `p=44.6%`). The held-out companion `tests/honcho_plugin/test_peers_map.py` was lower (`n=7`, `p=10.8%`) but still in the top ten overall and top three tests.

The path baseline suggests the Honcho test directory but not the peers-map suite. The historical list is noisy, yet concise support plus commit pointers is useful `material`; it does not need to assert that every result must change.

## `check-change`: defer

### Hidden companions are easy to recover

If the six locale companions are hidden from [`27dfbe3`](https://github.com/NousResearch/hermes-agent/commit/27dfbe397f849116c116e82e189499f747f62032), all six appear in the `en.ts` relation ranking above. This is the capability's best case.

### The same signal produces a high-confidence false warning

[`81de5af`](https://github.com/NousResearch/hermes-agent/commit/81de5afa44034d57522fc114a1e4209f818c2408) intentionally added the English-only keybinding label `view.cycleSidebarGrouping`. No other locale contains that key, including at current `main`. At replay time, however, `zh.ts` scored `n=505`, `p=96.4%`, followed by `types.ts`, `zh-hant.ts`, and `ja.ts`. A missed-change detector would loudly flag a valid change.

This cannot be repaired with a higher support threshold: the false warning is the strongest relation observed. Keep the underlying relation material, but defer any command that labels an absent companion as a mistake until it has intent-aware evidence beyond co-change history.

## `tests`: keep

### Cron source to exact suites

For [`c134199`](https://github.com/NousResearch/hermes-agent/commit/c134199886b2dad7358a1ad3707679b6292edd88), `hermes_cli/cron.py` mapped to both held-out test files:

- `tests/hermes_cli/test_cron.py`: `n=14`, `p=15.1%`, first test result;
- `tests/hermes_cli/test_cron_satellite_diagnostics.py`: `n=9`, `p=9.7%`, second test result.

The same-name baseline finds `test_cron.py`; it does not identify the satellite-diagnostics suite. The history adds a specific test target while retaining the commits behind the relation.

### Honcho source to behavioral suite

For [`476dbfe`](https://github.com/NousResearch/hermes-agent/commit/476dbfed3a76b85985674dc30e484de79859955f), the changed `test_peers_map.py` ranked third among tests behind `test_cli.py` and `test_session.py`. This is useful as a bounded candidate list, not as a single predicted test.

A rename shows the required ceiling: after [`d10bb2a`](https://github.com/NousResearch/hermes-agent/commit/d10bb2ab6fb9e16028070bf7075284a7b0476872) moved root tests to mirrored directories, the old `tests/test_browser_vault.py` still dominated the historical ranking for [`5fff41e`](https://github.com/NousResearch/hermes-agent/commit/5fff41e52ecc7bfb526d25d8ba3bfb96d67bb6d3) (`n=8`, `p=88.9%`). A current-tree filter or rename continuity is mandatory; the obvious mirrored path is better immediately after a move.

## `risk`: drop

### Hermes: churn highlights support files, not the novel change

Before [`27dfbe3`](https://github.com/NousResearch/hermes-agent/commit/27dfbe397f849116c116e82e189499f747f62032), the repository's highest eligible touch counts were `gateway/run.py` (1,763), `run_agent.py` (1,203), and `cli.py` (1,107). In the held-out feature, translation files ranked as high as 8th, while the substantive right-sidebar files ranged from rank 1,092 to 6,223 or had no history. Frequency would describe the locale registry as riskier than the new behavior.

### Rust: long history amplifies rollup and registry churn

The bounded review used [`28e8a8c`](https://github.com/rust-lang/rust/commit/28e8a8c81bf3b37909edac6c2a76e56f30cd492f), a seven-PR rollup changing 441 files. `compiler/rustc_span/src/symbol.rs` had 1,735 prior reachable touches (928 on the first-parent chain) and ranked first in the 100,000-mainline-commit probe window. That fact does not explain which of the seven changes is dangerous; the merge body and diff do.

The obvious command `git rev-list --count <commit>^ -- <path>` yields the same fact more directly. Raw churn is traceable but not useful enough `material`, and calling it change risk overstates what it knows.

## `clusters`: drop

Across the older Hermes history, the strongest depth-two groups were:

- `gateway/run.py` ↔ `tests/gateway`: `n=1,099`, minimum directional confidence 33.7%;
- `cron/scheduler.py` ↔ `tests/cron`: `n=299`, 58.3%;
- `plugins/memory` ↔ `tests/honcho_plugin`: `n=141`, 35.0%;
- `pyproject.toml` ↔ `uv.lock`: `n=121`, 48.2%.

These are real, but directory and filename inspection already reveals each boundary. Grouping also discards the exact path/commit detail that made `related` and `tests` useful. A separate cluster capability would duplicate the relation graph while returning less actionable material.

## Reproduction

```bash
target=c134199886
format='@@@%H%x09%aI%x09%s'
{
  git -C <hermes-clone> log -1 --diff-merges=first-parent \
    --format="$format" --name-only "$target"
  git -C <hermes-clone> log --diff-merges=first-parent \
    --format="$format" --name-only "$target^1"
} > hermes-case.log
python prototypes/issue-4/replay.py self-check
python prototypes/issue-4/replay.py case hermes-case.log "$target" \
  --seed hermes_cli/cron.py --train 40000
python prototypes/issue-4/replay.py clusters hermes-case.log \
  --train 40000 --depth 2 --support 10
```
