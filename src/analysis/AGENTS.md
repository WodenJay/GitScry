# Feature Ownership

- Keep each feature's execution workflow, material assembly, and exclusive result types together under `capabilities`.
- Use `query` for request dispatch and shared generation/scope lifecycle. Shared reading and scoring belong in `retrieval`.
- Keep history traversal separate from material eligibility; when a feature requires complete target history, apply scope filtering to results rather than prematurely limiting traversal.
- Resolve cache-backed default targets from the published cache generation; repository HEAD can be newer than the indexed history.