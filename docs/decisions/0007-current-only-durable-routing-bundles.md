# Decision 0007: Current-only durable routing-image bundles

Status: accepted

RocksDB confirmed records remain the sole durable graph source of truth. A routing-image bundle is a complete immutable, rebuildable index. PathHydra maintains one current layout and deliberately has no format magic, schema version, compatibility reader, migration path, graph revision counter, or caller-pinned historical-image API before the first release.

The manifest declares the exact numeric and tie policy identifiers, fixed element widths, counts, file lengths and BLAKE3 checksums, partition ranges, and partition checksums. All integers and floating-point bits are little-endian and decoded field-by-field. BLAKE3 1.8.2 is pinned; its Apache-2.0/CC0 licensing and maintained implementation are suitable for integrity checking. A checksum proves byte integrity, never graph identity.

One RocksDB metadata record stores a relative child bundle name and the exact 32-byte manifest checksum. Every confirmed graph-changing batch deletes that record atomically. Candidate-only writes do not. Publication holds the catalog confirmed-scan guard, completes and synchronizes a temporary child directory, validates it through the production reader, renames it within the image root, writes the pointer, constructs an execution representation, and publishes once. Requests own an immutable execution image, so a replacement cannot change their bytes.

| Last completed action | Restart behavior |
| --- | --- |
| Confirmed mutation | Pointer is absent; rebuild from confirmed records. |
| Partial temporary files | Pointer is absent; ignore the exact temporary child. |
| Temporary bundle validation | Pointer is absent; ignore the temporary child. |
| Final rename | Pointer is absent; ignore the unreferenced final child. |
| Pointer commit | Validate and open that exact final bundle. |
| Process dies before in-memory swap | Restart still opens the referenced valid bundle. |
| Later confirmed mutation | Its batch deleted the pointer; the older topology cannot become current. |

A graph revision counter is unnecessary: confirmed mutation invalidation is in the graph's own atomic batch, while compilation and pointer publication are serialized by the same confirmed-scan guard. Missing, corrupt, or omitted bundle files are cache loss and cause pointer clearing plus a RocksDB rebuild; they are never authoritative backup data.
