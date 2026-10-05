# Coding Standards

For each changed file, read and apply the scoped `AGENTS.md` files that cover its path. No need to read scoped `AGENTS.md` files for paths that are not being modified.

- `docs/**` → [`docs/AGENTS.md`](docs/AGENTS.md)
- `src/**` → [`src/AGENTS.md`](src/AGENTS.md)
- `src/analysis/**` → [`src/analysis/AGENTS.md`](src/analysis/AGENTS.md)
- `src/cache/**` → [`src/cache/AGENTS.md`](src/cache/AGENTS.md)
- `src/cli/**` → [`src/cli/AGENTS.md`](src/cli/AGENTS.md)
- `src/git/target/**` → [`src/git/target/AGENTS.md`](src/git/target/AGENTS.md)
- `tests/**` → [`tests/AGENTS.md`](tests/AGENTS.md)

Scoped instructions are cumulative. For example, a change under `src/analysis/**` must follow both `src/AGENTS.md` and `src/analysis/AGENTS.md`.