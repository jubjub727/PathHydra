# Durable Graph Store Layout

The default-column-family record `active-routing-image` is rebuildable index metadata, not graph state. Its current encoding is a little-endian `u32` UTF-8 relative-child-name length, the exact relative child name, and a 32-byte manifest checksum. Confirmed node, relation, or edge promotion and confirmed edge/node deletion remove it in the same RocksDB batch. Provisional candidate insertion does not.

Routing bundles may be omitted from backup. On restore, a missing referenced child or any checksum/semantic failure clears the pointer and rebuilds from confirmed records. There are intentionally no schema markers, compatibility layouts, graph revisions, or migrations before the first release.

## Routing-image bundle

The configured routing-image root contains immutable children named by the
publisher. A completed child contains exactly:

```text
manifest.bin
identities.bin
source-directory.bin
topology.bin
evidence.bin
```

`identities.bin` stores ascending stable node and relation IDs.
`source-directory.bin` stores the dense-source segment ranges and fixed segment
descriptors. `topology.bin` stores independently checksummed structure-of-arrays
partitions (destinations, relation indexes, and canonical base-weight bits).
`evidence.bin` stores the corresponding stable edge IDs. `manifest.bin` declares
the routing policies, field widths, counts, exact file lengths and checksums,
partition ranges, and partition checksums. Every field is decoded from an
explicit little-endian integer or floating-point bit pattern; Rust layout and
platform `usize` values are never persisted.

The streaming compiler holds the catalog's confirmed-scan guard, resident ID
tables and source directory, and one bounded partition buffer. It never builds
`ConfirmedGraphRecords` or all adjacency arrays in the production path. It
synchronizes the files and temporary directory, validates the bundle through
the production reader, renames the child within the image root, and only then
commits `active-routing-image`. Startup validates that exact child and pointer
checksum without rescanning RocksDB.

Confirmed mutation, bundle compilation, and pointer publication are serialized.
Every confirmed graph-changing batch removes the pointer. Therefore a crash can
leave an unreferenced temporary or final child, but cannot make incomplete or
pre-mutation bytes current. Startup removes only recognized unreferenced child
directories; it never applies a broad recursive cleanup path.

Each admitted request owns an immutable bundle lease. Replaced children enter a
count- and byte-bounded retirement queue and are removed only after all leases
expire. A Windows sharing violation remains a visible, retryable retirement
failure. Operators may omit the entire routing-image root from backup. Restore
the RocksDB directory, then allow the default startup policy to rebuild, or use
`StartupBundlePolicy::RequireValidBundle` to keep routing unavailable until an
operator calls `GraphEngine::rebuild_routing_image`.

## Batched confirmed-record reads

`Catalog::confirmed_records_by_id` adds no durable key space. It holds the existing catalog write mutex for one batch, deduplicates requested physical node and edge reads, and fetches the exact confirmed relation-kind record for every found edge. Missing requested IDs are ordinary missing results. A found edge with a missing endpoint or relation kind is corruption. Provisional candidates are never consulted.

This document describes the current RocksDB layout for exact node and
relation-kind identities, opaque node payloads, provisional candidates,
confirmed directed edges, and adjacency indexes. PathHydra is unreleased, so
the layout is changed in place when the data model changes. There are no schema
markers, compatibility records, or migration paths.

## Database layout

One RocksDB database contains the default column family and nine named column
families:

| Column family | Key | Value |
| --- | --- | --- |
| `default` | fixed metadata name | `u64` next-ID counter |
| `candidates` | candidate ID | node, relation-kind, or edge candidate |
| `nodes` | node ID | embedded ID, exact name, and opaque payload |
| `node_names` | encoded exact node name | node ID |
| `relation_kinds` | relation-kind ID | embedded ID, exact name, provisional-reference count, confirmed-edge count |
| `relation_names` | encoded exact relation-kind name | relation-kind ID |
| `edges` | edge ID | canonical directed edge record |
| `outgoing_edges` | source node ID, edge ID | edge ID |
| `incoming_edges` | destination node ID, edge ID | edge ID |
| `relation_popularity` | complemented total, complemented confirmed count, relation-kind ID | relation-kind ID |

The default column family contains `next-candidate-id`, `next-node-id`,
`next-relation-id`, and `next-edge-id`. New catalogs initialize every next ID
to 1. IDs are never reused or inferred from record contents.

## Common encodings

- Every numeric ID key or value is an eight-byte unsigned big-endian integer.
- An exact-name key or field is a four-byte unsigned big-endian byte length
  followed by the exact UTF-8 bytes.
- A payload field is a four-byte unsigned big-endian length followed by opaque
  bytes. Payloads are limited to 16 MiB. Empty and non-UTF-8 payloads are valid.
- A base weight is the canonical four-byte, big-endian IEEE-754 binary32 bit
  pattern described in [Decision 0001](decisions/0001-base-weight.md).

Decoders consume the entire key or value. They reject truncation, trailing
bytes, invalid UTF-8 names, unknown candidate kinds, noncanonical weights, and
disagreement between a numeric key and an ID embedded in its value. Names are
never trimmed, case-folded, normalized, corrected, aliased, or merged.

## Candidate values

All candidate values start with a one-byte kind followed by the embedded
eight-byte candidate ID.

| Kind | Tag | Remaining fields |
| --- | --- | --- |
| Node | `1` | exact name, opaque payload, incoming candidate-reference `u64` |
| Relation kind | `2` | exact name, incoming candidate-reference `u64` |
| Edge | `3` | source node reference, destination node reference, relation-kind reference, base weight |

An edge candidate is provisional. It has no edge ID, canonical edge record, or
adjacency entry. Each reference is a one-byte tag (`1` confirmed, `2`
candidate) and one big-endian eight-byte stable ID. Node references therefore
carry a `NodeId` or node `CandidateId`; relation references carry a
`RelationId` or relation-kind `CandidateId`. Request-local positions are never
durable. The incoming count on node/relation-kind candidates equals the exact
number of durable edge-candidate reference fields that name that candidate.

Insertion requires every confirmed identity to exist and every local
reference to identify the correct entry kind. Node deletion refuses while a
provisional edge directly references that confirmed node. Strict open and
verification reject missing, consumed, or mistyped candidate dependencies.

## Confirmed record values

- A node value is the embedded node ID, exact name, and payload.
- A relation-kind value is the embedded relation-kind ID, exact name, an
  eight-byte provisional direct-reference count, and an eight-byte confirmed
  canonical-edge count. Their checked sum must fit `u64`.
- An edge value is the embedded edge ID, source node ID, destination node ID,
  relation-kind ID, and base weight.

Edge IDs identify individual directed edges. Parallel edges, identical-looking
edges, and self-edges each have independent IDs and canonical records.

## Relation-kind popularity index

Every confirmed relation kind has exactly one 24-byte popularity key:

```text
big_endian(!total_reference_count)
|| big_endian(!confirmed_edge_count)
|| big_endian(RelationId)
```

Its value is the same `RelationId` as eight big-endian bytes. RocksDB forward
iteration yields total use descending, confirmed use descending, then stable
ID ascending. Zero-use kinds are indexed after used kinds. Candidate and edge
mutation replaces the old key and relation record atomically. Verification
independently recomputes provisional counts from edge candidates, incoming
candidate counts from candidate references, confirmed counts from canonical
edges, and the complete popularity key set.

## Adjacency indexes and invariants

An adjacency key is exactly 16 bytes: an eight-byte node ID followed by an
eight-byte edge ID. Big-endian fixed-width fields make every incident edge for
one node a bounded prefix read. The value repeats the eight-byte edge ID.

The `edges` record is authoritative. Every canonical edge must have exactly
one outgoing entry keyed by `(source, edge ID)` and exactly one incoming entry
keyed by `(destination, edge ID)`. Every index value repeats that edge ID and
must resolve back to a canonical record with the indexed endpoint. A self-edge
therefore has one entry in each index, not two entries in either index.

On open, PathHydra validates all canonical edges, both endpoint nodes, the
relation kind, the base weight, both adjacency directions, all candidates,
exact-name mappings, and next-ID counters. Malformed or inconsistent
structures are returned as typed errors; open never repairs, drops,
deduplicates, or merges them.

Ordinary batches explicitly keep the WAL enabled and do not request a device
sync. Checkpoint and shutdown hold the catalog mutation boundary, synchronize
the WAL, synchronously flush every current column family, and only then report
durability. Checkpoint uses RocksDB's checkpoint API and includes provisional
and confirmed records while omitting rebuildable routing files. Offline restore
copies only into a fresh admitted destination, validates the complete current
layout under caller work/time bounds, clears a stale routing pointer, and then
uses the production engine to rebuild and smoke the bundle.

The selected database options use four background jobs. The outgoing and
incoming adjacency families use an eight-byte fixed prefix extractor; all
other family compression, checksums, write buffers, and table behavior retain
the current RocksDB defaults. Structured metrics report per-family availability
rather than converting an unsupported property to zero. Catalog-owned
maintenance counters and process-owned offline restore counters are kept
separate so a restore remains observable after its fresh destination opens.
Checkpoint concurrency refusals count as failed checkpoint attempts. Explicit
compaction follows background/write-stop observation with a synchronous
durability probe; a storage-exhausted probe is reported as typed compaction
storage exhaustion, while a background failure without that capacity evidence
remains a distinct background failure.

## Atomic mutations

Candidate batch insertion validates and encodes the complete bounded request,
allocates one contiguous candidate-ID range, resolves local positions to those
IDs, aggregates dependency/provisional counts, and writes all candidates,
count/index replacements, and the final candidate counter in one `WriteBatch`.
Singleton insertion uses a one-entry batch.

Batch confirmation preflights a unique dependency-complete selection. It
simulates exact-name resolution and all node/relation/edge ID allocation before
writing. One `WriteBatch` creates every new record/name/adjacency entry,
transfers provisional use to confirmed use, replaces popularity entries,
removes every selected candidate, stores final counters, and invalidates the
routing pointer only when topology changed. Duplicate-name-only confirmation
still consumes all selected candidates but does not invalidate routing.

An incomplete dependency closure, missing identity, counter overflow,
allocation refusal, response-limit refusal, encoding error, or RocksDB failure
changes no candidate, counter, confirmed record, index, or routing pointer.

Edge deletion verifies and deletes the canonical record and its two exact
adjacency entries in one batch. Node deletion uses the two bounded adjacency
prefixes, deduplicates self-edges by edge ID, validates every canonical/index
relationship, and atomically removes all incident edge representations, the
node, and its exact-name mapping. Each affected relation count is decremented
once per deduplicated edge. An affected relation kind whose provisional and
confirmed counts both become zero is removed with its exact-name and popularity
entries; the removed stable IDs are returned. Unrelated zero-use kinds are not
swept. Provisional candidates are not deleted.
