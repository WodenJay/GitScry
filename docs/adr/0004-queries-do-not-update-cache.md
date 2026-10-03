# Query commands do not update the cache

GitScry query commands operate on the available cache and do not implicitly populate, refresh, repair, or extend it, even when doing so could improve coverage or results. Keeping cache maintenance separate makes query cost and side effects predictable; missing or incomplete cached history should be reported rather than silently resolved through a cache update.

An exception is allowed only when updating the cache is intrinsic to the feature's purpose, such as conflicts, not merely an optimization or a way to make an ordinary query succeed. Such a feature must make its cache-updating behavior explicit. Ordinary queries leave cache maintenance to an explicit user operation.
