# Durable Graph Store Format

This document fixes RocksDB storage format version 2. It covers exact node and
relation-kind identities, opaque node payloads, provisional candidates,
confirmed directed edges, adjacency indexes, and the migration from version 1.
It does not define routing snapshots, hydration, or caller-owned subgraphs.

## Database layout

One RocksDB database contains the default column family and eight named column
families:

| Column family | Key | Value |
| --- | --- | --- |
| `default` | fixed metadata name | format marker or versioned `u64` metadata |
| `candidates` | candidate ID | node, relation-kind, or edge candidate |
| `nodes` | node ID | embedded ID, exact name, and opaque payload |
| `node_names` | encoded exact node name | node ID |
| `relation_kinds` | relation-kind ID | embedded ID and exact name |
| `relation_names` | encoded exact relation-kind name | relation-kind ID |
| `edges` | edge ID | canonical directed edge record |
| `outgoing_edges` | source node ID, edge ID | versioned edge ID |
| `incoming_edges` | destination node ID, edge ID | versioned edge ID |

The default column family has `storage-format`, `graph-version`,
`next-candidate-id`, `next-node-id`, `next-relation-id`, and `next-edge-id`
records. New catalogs start at graph version 0 with every next ID set to 1.
IDs are never reused or inferred from record contents.

## Common encodings

- The `storage-format` value is the single byte `2`.
- Every other value starts with the single-byte record version `2`.
- Every numeric ID key is an eight-byte unsigned big-endian integer.
- Every versioned `u64` value is its version byte followed by eight unsigned
  big-endian bytes.
- An exact-name key or field is a four-byte unsigned big-endian byte length
  followed by the exact UTF-8 bytes.
- A payload field is a four-byte unsigned big-endian length followed by opaque
  bytes. Payloads are limited to 16 MiB. Empty and non-UTF-8 payloads are valid.
- A base weight is the canonical four-byte, big-endian IEEE-754 binary32 bit
  pattern described in [Decision 0001](decisions/0001-base-weight.md).

Decoders consume the entire key or value. They reject truncation, trailing
bytes, invalid UTF-8 names, unknown versions or kinds, noncanonical weights,
and disagreement between a numeric key and an ID embedded in its value. Names
are never trimmed, case-folded, normalized, corrected, aliased, or merged.

## Candidate values

All candidate values start with the version, a one-byte kind, and the embedded
eight-byte candidate ID.

| Kind | Tag | Remaining fields |
| --- | --- | --- |
| Node | `1` | exact name, opaque payload |
| Relation kind | `2` | exact name |
| Edge | `3` | source node ID, destination node ID, relation-kind ID, base weight |

An edge candidate is provisional. It has no edge ID, canonical edge record, or
adjacency entry. Its referenced confirmed records may disappear while it is
provisional; promotion checks them again.

## Confirmed record values

- A node value is version, embedded node ID, exact name, and payload.
- A relation-kind value is version, embedded relation-kind ID, and exact name.
- An edge value is version, embedded edge ID, source node ID, destination node
  ID, relation-kind ID, and base weight.

Edge IDs identify individual directed edges. Parallel edges, identical-looking
edges, and self-edges each have independent IDs and canonical records.

## Adjacency indexes and invariants

An adjacency key is exactly 16 bytes: an eight-byte node ID followed by an
eight-byte edge ID. Big-endian fixed-width fields make every incident edge for
one node a bounded prefix read. The value is a versioned `u64` containing the
same edge ID.

The `edges` record is authoritative. Every canonical edge must have exactly
one outgoing entry keyed by `(source, edge ID)` and exactly one incoming entry
keyed by `(destination, edge ID)`. Every index value repeats that edge ID and
must resolve back to a canonical record with the indexed endpoint. A self-edge
therefore has one entry in each index, not two entries in either index.

On open, PathHydra validates all canonical edges, both endpoint nodes, the
relation kind, the base weight, and both adjacency directions. It also validates
all candidates and exact-name mappings. Unknown formats and malformed or
inconsistent structures are returned as typed errors; open never repairs,
drops, deduplicates, or merges them.

## Atomic mutations

Candidate insertion writes only the candidate and advances the candidate
counter. It does not increment the graph version.

Successful promotion of a node or relation kind writes its canonical record
and exact-name mapping, removes the candidate, advances its stable-ID counter,
and increments the graph version in one `WriteBatch`.

Successful edge promotion first verifies both confirmed endpoint nodes and the
confirmed relation kind. One `WriteBatch` then writes the canonical edge and
both adjacency entries, removes the candidate, advances `next-edge-id`, and
increments the graph version. A failed promotion changes none of them.

Edge deletion verifies and deletes the canonical record and its two exact
adjacency entries, then increments the graph version, in one batch. Node
deletion uses the two bounded adjacency prefixes, deduplicates self-edges by
edge ID, validates every canonical/index relationship, and atomically removes
all incident edge representations, the node, its exact-name mapping, and
increments the graph version once. Provisional candidates are not deleted.

## Version 1 migration

Opening a version 1 exact-identity catalog creates the three version 2 column
families and performs one atomic, idempotent migration batch:

1. decode every version 1 candidate, confirmed record, name mapping, and
   metadata value with the version 1 codec;
2. re-encode them as version 2, adding an empty payload to every legacy node
   and node candidate;
3. preserve every candidate, node, and relation-kind ID and every exact name;
4. initialize `next-edge-id` to 1;
5. write the format marker as version 2 after all conversions have been added
   to the batch.

RocksDB applies the batch atomically, so a process failure leaves either the
complete version 1 catalog or the complete version 2 catalog. Reopening safely
repeats the migration only when the version 1 marker remains. Malformed legacy
records and unknown format versions are reported and leave the marker and all
records unchanged.
