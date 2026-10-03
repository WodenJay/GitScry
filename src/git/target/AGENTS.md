# Target Preparation

- Keep feature-specific target pinning with that feature's target module. Share revision/path/tree/blob and blame mechanics where reused.
- Target preparation resolves Git facts. Cache eligibility and material selection belong in analysis, even when they consume the pinned target.
- Preserve degraded-history outcomes: shallow history, missing objects, and unavailable blame can yield warnings or partial attribution rather than an unconditional failure.
