# Hydration

Hydration resolves caller-specified stable handles against current confirmed catalog state. It never consults provisional candidates and never implies a historical storage snapshot.

`Catalog::confirmed_records_by_id` holds the catalog write mutex across one deduplicated batch. Requested nodes and edges are physically read once; every found edge's endpoints and exact relation-kind record are checked. The engine then restores caller order and duplicates. Ordinary absent requested IDs become `Missing`; a found edge with an absent dependency is structural corruption.

Generic hydration may omit a relation profile. Such edges are explicitly unprofiled and have no invented effective weight. When supplied, the whole profile is validated against the current active image before catalog reads and is returned canonically. Each edge is `Disabled` or `Enabled` with its exact multiplier and Decision 0002 effective weight.

Path hydration accepts one `RoutingResponse` and destination position. It takes the path, profile, numeric policy, and tie policy from that response, fetches all distinct handles in one current-state batch, and revalidates identity, direction, relation kind, base weight, multiplier, effective weight, continuity, and exact summed logical distance. It does not repack an old profile against a newer image. If current deletion removed any evidence, the error lists all unavailable node and edge IDs; no substitute path is selected.

Subgraph hydration uses the same batch path. Stable node and edge order is preserved, found edges must match caller-owned endpoint evidence, and missing records produce an explicitly incomplete result without changing the input subgraph.
