# Never introduce an LLM into GitScry

GitScry is a historical-material tool for agents, not an agent or a reasoning service. GitScry will never introduce an LLM dependency or integration, whether local or remote, required or optional, for generation, classification, reranking, or judging historical relationships. The consuming agent already supplies reasoning; embedding that responsibility inside GitScry would duplicate it and add model-dependent behavior, cost, and operational complexity.

This boundary does not prohibit non-generative embedding models used for semantic retrieval, such as the fixed MiniLM encoder covered by ADR-0001. Such retrieval locates historical material; it does not use an LLM to interpret that material or reach conclusions for the agent. When a feature needs interpretive judgment, GitScry should expose traceable material for the consuming agent rather than add an LLM step.
