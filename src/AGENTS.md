# Placement

- Put feature analysis in `analysis`; let `app` coordinate commands rather than assemble analysis workflows.
- Keep Git mechanics in `git`, persisted-data access in `cache`, and output wording/serialization in `render`. A CLI feature may span these owners; its command name alone does not determine placement.
- Keep feature-only result types with their feature. Promote types or helpers to shared modules when multiple consumers share their meaning, not merely their shape.
