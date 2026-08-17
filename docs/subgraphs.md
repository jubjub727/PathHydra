# Caller-owned subgraphs

`pathhydra-subgraph::Subgraph` is a deterministic, in-memory set of stable handles. Nodes use a standard-library ordered set. Edges use an ordered map from `EdgeId` to source and destination evidence. Ordering makes inspection stable; it does not change graph semantics.

Adding an edge inserts both endpoints. Repeating a node or an edge with the same endpoints is idempotent. Reusing an edge ID with different endpoints is a typed conflict. Parallel edges remain distinct and a self-edge is stored once.

`add_path` validates origin, destination, continuity, and every edge identity before mutation. `union` likewise validates all conflicts first, so both operations are atomic to the caller. Removing a node removes every included incoming and outgoing edge. Removing an edge preserves isolated nodes.

`SubgraphHandles` exports ordered node IDs and edge endpoint evidence.
`SubgraphHandlesDto` carries the same structure across the consumer boundary,
where Decision 0012's bounded canonical JSON preserves order, parallel/self-edge
identity, and endpoint evidence. The container owns no catalog or engine
reference and has no interior mutation. Every edit affects only the caller's
value. The engine hydrates records but never decides which paths to union or how
to compose a final inference graph.
