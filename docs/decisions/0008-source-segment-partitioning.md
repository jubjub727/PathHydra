# Decision 0008: Source-segment partitioning and bounded execution

Status: accepted

Dense nodes use ascending stable `NodeId` order. Each source's outgoing relations retain ascending stable `EdgeId` order. A partition contains one or more source segments; a high-degree source may span consecutive segments and partitions. The resident source directory maps every dense source to its ordered segment range, and concatenation exactly reproduces resident adjacency.

Partitions are immutable and independently checksummed. The configured target closes ordinary partitions at source boundaries; the hard maximum can split a source and must fit the smallest nonempty segment. The fixed identity tables, relation IDs, source directory, and one search's global state have separate admission limits.

CPU state remains in host memory while topology moves through a byte- and entry-bounded shared cache. Presence, eviction, and request interleaving affect only performance. Expansion still counts each relation immediately before profile evaluation and uses the same separate binary64 multiplication/addition and stable predecessor tuple. Pending reads are unfinished work; pinned entries cannot be evicted. Discarded bytes remain reloadable from the exact bundle owned by the request.

Conventional checked file reads and explicit accelerator copies are the transport baseline. A future transport can be compared only after measurements show file transport dominates; it cannot change partition selection or exact semantics.
