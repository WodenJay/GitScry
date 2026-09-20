# Issue 19 trace-fix replay

Status: bounded replay of the accepted trace-fix case from issue #3. The source case is
Rust compiler PR #160605 → issue #160994 → corrective PR #161268. The replay is local
and deterministic when the referenced commits are present; GitScry does not fetch PR
metadata.

## Rust case

Fix commit: `39b6dbec0617230ba45954f27fd6ff4431b44674`.
The fix-parent deleted-line blame in
`compiler/rustc_next_trait_solver/src/solve/assembly/mod.rs` attributes the moved
solver check to `6c28fdda79b9d1be22c8da03342beb896b6b2898`, whose message explains the
performance reorder. The corrective fix message names the failure and the related PRs.
The bounded verification is:

```bash
git blame -L 565,572 39b6dbec0617230ba45954f27fd6ff4431b44674^ -- \
  compiler/rustc_next_trait_solver/src/solve/assembly/mod.rs
git show --stat --oneline 6c28fdda79b9d1be22c8da03342beb896b6b2898
git show --stat --oneline 39b6dbec0617230ba45954f27fd6ff4431b44674
```

The expected local chain is:

```text
6c28fdda79b9d1be22c8da03342beb896b6b2898  introducing performance reorder
#160994                                  observed solver regression
39b6dbec0617230ba45954f27fd6ff4431b44674  corrective fix
```

`trace-fix` obtains the first and third facts from local Git. The issue/PR bridge is
reported as unavailable when it is not represented in local commit messages; it is not
invented as a local commit. The fix-parent blame is the important difference from plain
HEAD blame, which would attribute the changed lines to the fix itself. Rename/copy
history and first-parent merge boundaries are rendered as basis/warnings rather than
silently treated as proof.

## Offline process fixture

The process-level fixture
`tests/trace_fix.rs::trace_fix_uses_only_pre_fix_failure_context_and_does_not_invent_one`
replays the same bounded shape with local commits: an introducing change, a pre-fix
message-reference failure, the fix, and a post-fix message-reference commit. It runs:

```bash
cargo test --test trace_fix \
  trace_fix_uses_only_pre_fix_failure_context_and_does_not_invent_one -- --exact
```

The assertions require the introducing, failure, and fix citations, exclude the
post-fix commit, and reject a fabricated “observed failure” basis when the fix has no
stated reason. This keeps the acceptance check offline while preserving the real Rust
case as the manual bounded replay record.
