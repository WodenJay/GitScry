# Contributing to GitScry

Contributions are welcome. Bug reports, documentation improvements, test cases, and code contributions all help make GitScry better.

For larger features or changes to existing behavior, please open an issue first so we can discuss the approach before implementation.

## Ways to contribute

You can contribute by:

- Reporting bugs
- Suggesting features or improvements
- Improving documentation
- Adding useful tests or benchmark cases
- Fixing bugs
- Implementing approved features

## Before you start

For small fixes, documentation changes, and straightforward improvements, feel free to open a pull request directly.

For larger changes, such as:

- Adding a new command
- Changing existing command behavior
- Introducing a major architectural change
- Significantly changing output formats or ranking behavior

please open an issue first and describe the problem you want to solve and the approach you have in mind.

This helps avoid duplicated work and makes it easier to agree on the intended behavior before implementation.

## Development setup

Clone the repository:

```bash
git clone https://github.com/WodenJay/GitScry.git
cd GitScry
```

Build the project:

```bash
cargo build
```

Run the test suite:

```bash
cargo test
```

## Before submitting a pull request

Please make sure the following checks pass:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

If your change affects behavior, add or update tests where appropriate.

Avoid adding tests that only reproduce implementation details without validating meaningful behavior.

## Pull requests

Keep pull requests focused. A pull request should ideally address one problem or one logical change.

A good pull request should explain:

- What changed
- Why the change is needed
- How the change was tested
- Any behavior or output that changed

If the pull request resolves an existing issue, reference it in the description, for example:

```text
Fixes #123
```

Please avoid unrelated refactoring in the same pull request unless it is necessary for the change.

## Reporting bugs

When reporting a bug, include enough information to reproduce and investigate it.

Useful information includes:

```text
GitScry version:
Operating system:
Repository or reproduction case:
Command:
Actual output:
Expected output:
```

For history-dependent commands such as `why`, `regression`, and `trace-fix`, please include the relevant repository, commit, path, revision, or other Git context when possible.

A minimal reproducible example is especially helpful.

## Feature requests

Feature requests should describe the problem or workflow first, rather than only proposing an implementation.

Please include:

- What you are trying to do
- Why the current behavior is insufficient
- An example of the expected workflow or output

This makes it easier to evaluate whether the feature fits GitScry and how it should behave.

## Documentation

Documentation fixes and improvements are welcome.

Examples include:

- Correcting inaccurate instructions
- Improving command examples
- Clarifying confusing behavior
- Adding examples for real-world workflows

## License

By contributing to GitScry, you agree that your contributions will be licensed under the repository's [MIT License](LICENSE).
