# Keep the local Web interface a read-only investigation layer

GitScry's optional `gitscry web` interface will make query-selected material easier to inspect and follow, rather than become a separate Git client or development workbench. It will bind one local repository per process, reuse the CLI's query implementation, and ship its page resources with GitScry without requiring a separate frontend runtime or CDN. This trades remote access, repository management and persistent investigation workspaces for a small local presentation layer whose query meaning remains consistent with the CLI.

## Consequences

- The primary workflow is querying in the browser and following returned material. All historical query capabilities, including those planned in open issues, receive basic presentation; lists/cards, timelines and local typed relationship views serve different result shapes rather than forcing everything into a repository-wide graph.
- Further queries are explicit user actions. Co-change, commit lineage and GitHub associations retain their distinct meanings; presentation must not turn a suspect or association into a causal conclusion.
- Historical detail can read local Git content only for commits covered by the published cache. Such detail is distinct from query-selected material and does not expand the historical query scope. Missing objects remain unavailable; the Web interface does not fetch objects or build/update the cache.
- The Web interface does not modify source files, Git state, cache contents or GitHub objects. Existing explicit GitHub-link retrieval and local aggregate usage statistics remain compatible with this repository-oriented read-only constraint; browser interactions do not silently become CLI invocations for statistics.
- The process listens only on loopback with session authorization and request-origin validation. It attempts to open a browser and prints the local access address; ending the process ends the Web instance. LAN exposure, remote hosting and background operation are excluded from the first version.
- Investigation records are retained only within the current session, without durable storage, saved workspaces or shareable investigation links. There is no first-version usage-statistics dashboard.

This records the accepted product scope discussed in issue [#99](https://github.com/WodenJay/GitScry/issues/99), using local code plus implemented open-issue plans as the design baseline. It is not a claim that the Web interface exists, and it does not settle individual frontend/server dependencies or the still-open detailed interaction contracts.
