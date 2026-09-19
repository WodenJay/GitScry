# Issue 16: bounded related/tests replay

Status: accepted replay evidence for the production `related` and `tests` queries.

The issue-4 probe used a bounded first-parent history window, deduplicated paths per
commit, and excluded commits touching more than 50 paths. This note records those bounded
results; production applies the mass-change boundary and ignores merge replays while the
prepared cache supplies its available history. The probe's accepted results were:

| Case | Historical signal | Path-only baseline | Production implication |
|---|---|---|---|
| `hermes_cli/cron.py` → `tests/hermes_cli/test_cron.py` | 14 co-changes, 15.1% of seed touches | Finds the mirrored test | Keep the traceable support count and proportion |
| `hermes_cli/cron.py` → `tests/hermes_cli/test_cron_satellite_diagnostics.py` | 9 co-changes, 9.7% of seed touches | Misses the satellite suite | Give non-mirrored test candidates a ranking bonus |
| `plugins/memory/honcho/cli.py` → `tests/honcho_plugin/test_peers_map.py` | 7 co-changes, third among test candidates | Does not identify the behavioral suite | Return a bounded candidate list, not a coverage claim |
| Test-directory rename | Historical old path remains strong after a move | Current tree exposes the new path | Filter absent paths and emit rename-continuity warning |

These cases show why history adds value over same-name inspection without justifying a
`check-change` command: the same strong relation can be an intentional partial change.
No change-obligation or completeness claim is made by either command.

The process tests in `tests/relations.rs` replay the corresponding properties locally:
multiple seeds merge into one candidate, a non-mirrored test path is retained, mass
changes and merge replays do not inflate support, deleted/renamed historical paths are
filtered from `tests`, and the fixed empty results remain stable.

Source: issue-4 prototype replay protocol and results (`prototype/issue-4-change-relations`,
commit `1bb27b1`), retained here as the bounded-history decision record.
