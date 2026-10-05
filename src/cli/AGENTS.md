# CLI Help Design

- Write English help as an operating manual: the help hierarchy must let users discover and use every command and option without external docs. JSON help covers output selection, not the data protocol.
- Keep help progressive through command levels. Root help contains usage, a command map with one-line descriptions, global options, and a `gitscry <command> --help` pointer. `-h` and `--help` show identical content at each level.
- Order subcommand help: purpose, usage, arguments, options, necessary mode/scope notes, examples. Explain each parameter once, in its argument or option entry; use notes for relationships between parameters.
- Keep information that changes how users choose or run a command: prerequisites, input rules, defaults, scope, mode compatibility, important limits, and side effects. Match actual behavior.
- Use concrete verbs and accurate result names. Remove promotion, disclaimers, implementation specifications, schema version matrices, and repeated explanations; add no help levels to house them.
- Keep CLI code simple and direct; reuse existing structure rather than adding abstractions for help text.
