# Decision 0008: Source-segment partitioning and bounded execution

Status: accepted

Dense nodes use ascending stable `NodeId` order. Each source's outgoing relations retain ascending stable `EdgeId` order. A partition contains one or more source segments; a high-degree source may span consecutive segments and partitions. The resident source directory maps every dense source to its ordered segment range, and concatenation exactly reproduces resident adjacency.

Partitions are immutable and independently checksummed. The configured target closes ordinary partitions at source boundaries; the hard maximum can split a source and must fit the smallest nonempty segment. The fixed identity tables, relation IDs, source directory, and one search's global state have separate admission limits.

CPU state remains in host memory while topology moves through a byte- and entry-bounded shared cache. Presence, eviction, and request interleaving affect only performance. Expansion still counts each relation immediately before profile evaluation and uses the same separate binary64 multiplication/addition and stable predecessor tuple. Pending reads are unfinished work; pinned entries cannot be evicted. Discarded bytes remain reloadable from the exact bundle owned by the request.

Conventional checked file reads and explicit accelerator copies are the selected transport. In the final 12-GiB regression, Frontier spent 18.13 of 50.67 seconds and Delta spent 36.09 of 80.11 seconds in partition scheduling/I/O; relation relaxation remained the larger stage in both. File transport was therefore material but not repeatably dominant, so no DirectStorage implementation was added. Any alternative requires a separate measured transport plan and cannot change partition selection or exact semantics.

CUDA uses the same directory. The resident mode remains preferred when the
complete topology fits. Otherwise global identity, profile, distance, bucket,
and frontier state remain resident while topology partitions pass through
byte- and slot-bounded device storage. Frontier completion includes every
source segment and all pending reads, copies, launches, and synchronization.
Delta-stepping repeats light-partition work through same-bucket closure, then
processes the heavy partitions named by the completed removed set. Both phases
derive partition groups from current-bucket sources and do not scan unrelated
partitions. Reversing the partition schedule is required to preserve exact CPU
distance bits.

Device entries move through explicit host-loading, copying, ready/in-use,
evicting, and failed states. Concurrent requests coalesce on the loading/copying owner.
Entries are immutable and reference-counted while used by launches, and every
launch records a CUDA completion event. A slot is reusable only after all users
release it, its events complete, and device-allocation release finishes outside
the cache mutex; allocation, copy, launch, event,
synchronization, and context-loss failures release host and device reservations
through that boundary. Context loss poisons the CUDA runtime but not the
acquired bundle. Permissive policy reruns the complete request on the matching
CPU representation, while explicit CUDA reinitialization creates a fresh
context and cache.
