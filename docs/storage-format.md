# Durable Graph Store Layout

## Batched confirmed-record reads

`Catalog::confirmed_records_by_id` adds no durable key space. It holds the existing catalog write mutex for one batch, deduplicates requested physical node and edge reads, and fetches the exact confirmed relation-kind record for every found edge. Missing requested IDs are ordinary missing results. A found edge with a missing endpoint or relation kind is corruption. Provisional candidates are never consulted.

This document describes the current RocksDB layout for exact node and
relation-kind identities, opaque node payloads, provisional candidates,
confirmed directed edges, and adjacency indexes. PathHydra is unreleased, so
the layout is changed in place when the data model changes. There are no schema
markers, compatibility records, or migration paths.

## Database layout

One RocksDB database contains the default column family and eight named column
families:

| Column family | Key | Value |
| --- | --- | --- |
| `default` | fixed metadata name | `u64` next-ID counter |
| `candidates` | candidate ID | node, relation-kind, or edge candidate |
| `nodes` | node ID | embedded ID, exact name, and opaque payload |
| `node_names` | encoded exact node name | node ID |
| `relation_kinds` | relation-kind ID | embedded ID and exact name |
| `relation_names` | encoded exact relation-kind name | relation-kind ID |
| `edges` | edge ID | canonical directed edge record |
| `outgoing_edges` | source node ID, edge ID | edge ID |
| `incoming_edges` | destination node ID, edge ID | edge ID |

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
| Node | `1` | exact name, opaque payload |
| Relation kind | `2` | exact name |
| Edge | `3` | source node ID, destination node ID, relation-kind ID, base weight |

An edge candidate is provisional. It has no edge ID, canonical edge record, or
adjacency entry. Its referenced confirmed records may disappear while it is
provisional; promotion checks them again.

## Confirmed record values

- A node value is the embedded node ID, exact name, and payload.
- A relation-kind value is the embedded relation-kind ID and exact name.
- An edge value is the embedded edge ID, source node ID, destination node ID,
  relation-kind ID, and base weight.

Edge IDs identify individual directed edges. Parallel edges, identical-looking
edges, and self-edges each have independent IDs and canonical records.

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

## Atomic mutations

Candidate insertion writes only the candidate and advances the candidate
counter.

Successful promotion of a node or relation kind writes its canonical record
and exact-name mapping, removes the candidate, and advances its stable-ID
counter in one `WriteBatch`.

Successful edge promotion first verifies both confirmed endpoint nodes and the
confirmed relation kind. One `WriteBatch` then writes the canonical edge and
both adjacency entries, removes the candidate, and advances `next-edge-id`. A
failed promotion changes none of them.

Edge deletion verifies and deletes the canonical record and its two exact
adjacency entries in one batch. Node deletion uses the two bounded adjacency
prefixes, deduplicates self-edges by edge ID, validates every canonical/index
relationship, and atomically removes all incident edge representations, the
node, and its exact-name mapping. Provisional candidates are not deleted.
