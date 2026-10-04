# GitScry Dogfooding

Whenever GitScry is used, briefly assess whether it meaningfully helped with the intended task.

Record a case only when it did not—for example, its output was not useful, it failed to provide needed information, or its behavior materially differed from what was reasonably expected. Do not record merely imperfect uses or create entries just for the sake of dogfooding.

## Template

### [Date] Short description

- **Command:** `gitscry ...`
- **Goal:** What I wanted to learn or accomplish.
- **Actual:** What GitScry returned or failed to do.
- **Expected:** What would have been useful instead.

<!-- DO NOT EDIT ABOVE THIS LINE. APPEND NEW ENTRIES BELOW. -->

## Entries

### 2026-10-04 Historical follow-up query

- **Command:** `gitscry followups 4f4720c --json`
- **Goal:** Inspect later descendants of a project history change for follow-up examples.
- **Actual:** No entries; only one commit was inspected. The published cache endpoint was `6c3ab614d18c4f0d57b9b7a17ec04297f5bc5bad`, and coverage reported newer reachable history missing.
- **Expected:** Cached descendant changes in the requested scope that could inform the implementation. The available cache did not include enough history.