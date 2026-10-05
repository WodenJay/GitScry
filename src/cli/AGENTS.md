# CLI Help Design

1. `gitscry --help` should provide only the high-level command map: available subcommands and a concise description of what each subcommand does.
2. Detailed options, arguments, defaults, and command-specific behavior should live in that subcommand's `--help`, not in the top-level help.
3. Each level of help should be self-contained for decisions at that level, but should not duplicate details owned by a deeper level.
4. When a user needs more detail about a specific command, direct them to `gitscry <command> --help` rather than expanding the parent help.
5. Keep help progressive: show only the information needed to choose the next command or action, and reveal implementation details only when the user drills down.
6. The complete `--help` hierarchy must cover all user-facing functionality: a user should be able to discover every feature and use case of GitScry through `gitscry --help` and the relevant subcommand `--help` pages alone.
7. Help text must match the actual CLI behavior. Every user-facing command, option, constraint, default, and important behavior described in `--help` must stay consistent with the implementation.
8. Keep CLI code simple and direct. Prefer the shortest clear implementation; avoid unnecessary abstractions, wrappers, and verbose logic.
