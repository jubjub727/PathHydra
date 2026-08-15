# Exact Identity Catalog Storage Format

This document fixes storage format version 1 for the exact identity catalog.
It does not define edges, routing snapshots, hydration, or subgraphs.

## Database layout

One RocksDB database contains the default column family and five named column
families:

| Column family | Key | Value |
| --- | --- | --- |
| `default` | fixed metadata name | versioned format marker or `u64` metadata |
| `candidates` | candidate ID | candidate kind, embedded ID, and exact name |
| `nodes` | node ID | embedded node ID and exact name |
| `node_names` | encoded exact node name | node ID |
| `relation_kinds` | relation ID | embedded relation ID and exact name |
| `relation_names` | encoded exact relation name | relation ID |

The default column family has `storage-format`, `graph-version`,
`next-candidate-id`, `next-node-id`, and `next-relation-id` records. New
catalogs start with graph version 0 and next IDs of 1.

## Encodings

- Every numeric ID key is an eight-byte, unsigned, big-endian integer.
- An exact-name key is a four-byte, unsigned, big-endian byte length followed
  by the exact UTF-8 bytes.
- Every value begins with the one-byte storage format version (`1`).
- A numeric value continues with an eight-byte, unsigned, big-endian integer.
- Candidate values continue with a one-byte kind (`1` for node, `2` for
  relation), the eight-byte embedded candidate ID, and an encoded exact name.
- Confirmed record values continue with the eight-byte embedded stable ID and
  an encoded exact name.

Decoders require canonical lengths and consume the entire key or value. They
reject truncation, trailing bytes, invalid UTF-8, unknown versions or kinds,
and disagreement between a numeric key and the ID embedded in its value.
Names are never trimmed, case-folded, normalized, corrected, or aliased.

## Atomic confirmation batch

Confirmation first verifies the durable exact-name mapping while holding the
catalog write mutex. A successful confirmation commits one RocksDB
`WriteBatch` containing all of the following:

1. create the confirmed record;
2. create its exact-name mapping;
3. delete the provisional candidate;
4. advance the relevant stable-ID counter;
5. increment the graph version.

The in-memory exact-name map is updated only after that batch commits.
Duplicate names and counter overflow return errors before the batch is written,
so the candidate remains provisional and the graph version is unchanged.
