# Decision 0010: Durable operations and current store layout

Status: accepted

One process owns a live RocksDB catalog path for mutation and routing-image
publication. Confirmed records and provisional candidates in that catalog are
the authoritative durable state. Routing-image bundles are complete immutable,
rebuildable indexes owned by Plan 06, not backup authority.

## Current first-release layout

Retain the default metadata family and the eight named families `candidates`,
`nodes`, `node_names`, `relation_kinds`, `relation_names`, `edges`,
`outgoing_edges`, and `incoming_edges`. Numeric keys are fixed-width big-endian
integers. Exact-name keys contain their byte length and exact UTF-8 bytes.
Outgoing and incoming adjacency each retain one 16-byte `(NodeId, EdgeId)` key
and one eight-byte `EdgeId` value per directed edge. The fixed eight-byte node
prefix is configured on both adjacency families.

This is one current, pre-release layout. There is no format marker, migration
reader, graph revision, compatibility layout, or packed durable adjacency.
Decision 0008 remains responsible for splitting high-degree topology in the
routing image; it does not change the durable adjacency representation.

The Plan 08 workload records sequential and randomized promotion, exact lookup,
both high-degree adjacency directions, random and cascading deletion, streaming
bundle build, restart validation, checkpoint/restore, and post-churn space. The
current small representative run and the repeatable larger command are recorded
in [RocksDB operations performance](../performance/rocksdb-operations.md).
Those results establish correctness and a reproducible baseline. They do not
show a material, cross-workload benefit for another column-family or key
layout, so no second layout is introduced.

## Options and durability

Retain the current narrow configuration:

- four RocksDB background jobs;
- the fixed prefix extractor only on outgoing and incoming adjacency;
- RocksDB's current leveled organization, checksums, write buffers, block
  behavior, and compression defaults otherwise;
- no global statistics ticker solely for this plan; unsupported counters are
  `Unavailable`, never zero.

Every graph, candidate, pointer, and maintenance write uses the WAL. Ordinary
commits do not request a device sync. A checkpoint and explicit engine shutdown
perform the documented synchronous WAL/memtable flush before reporting
durability assurance. The selected policy is exported as
`WalEnabledExplicitSync`.

This policy makes the distinction visible: an acknowledged ordinary write is
WAL-backed but is not a claim that the storage device completed a forced sync.
Callers needing a durability boundary use checkpoint or explicit shutdown.

## Backup, restore, and ownership

RocksDB checkpoint is the local backup contract. A checkpoint contains both
confirmed and provisional state and omits routing bundles by default. Restore
copies into a fresh destination, fully validates records, exact-name indexes,
adjacency, metadata, and counters, and clears an unusable routing pointer.
Routing is then rebuilt from confirmed records. Current-record hydration is
intentionally nonhistorical.

Database, image, checkpoint, restore, and scratch targets are resolved and
validated together beneath caller-selected roots. Filesystem roots, aliases,
equal or nested live/backup targets, nonempty fresh targets, and targets that
could overwrite an open database are rejected. No recovery command deletes or
replaces the source or live directory.

## Complete rebuild versus overlay

Retain complete immutable routing-image rebuilds for the first release. The
workload measures confirmed mutation and the following streaming complete
rebuild. The engine suite measures five samples for four mutation types at 256,
1,024, and 4,096 nodes. In the exact post-publication-oracle rerun at 4,096
nodes, p95 mutation was 49.35--80.29 ms and p95 blocked-route time was
48.46--79.31 ms; every result was correct. This meets the
selected local-interactive bounds of 110 ms mutation and 100 ms route blocking
for up to 4,096 nodes and 12,285 directed edges on the reference machine.
There is no graph revision API and no hidden overlay. If a target workload
exceeds those recorded bounds, work stops for a separate reviewed overlay plan
spanning routing, deletion, publication, GPU, and recovery semantics.

## Consequences

- Operators receive aggregate inspection, verification, metrics, checkpoint,
  restore, and dry-run reconciliation commands, never a RocksDB shell.
- Offline restore attempts are process-owned and remain observable separately
  from per-catalog counters after a fresh restored catalog opens.
- Backups are smaller and remain independent of rebuildable bundle bytes.
- One-entry adjacency write and deletion amplification is accepted and remains
  measurable.
- Explicit compaction is limited to one fixed-scope operation over every current
  catalog family. No raw family/range controls are public.

At scale 10,000/100 samples, the retained four-job plus eight-byte adjacency
prefix configuration improved p95 streaming build from 135.82 to 123.88 ms and
restore validation from 289.30 to 267.67 ms compared with the removed
all-default alternative. The alternative slightly improved some point writes
and one cascade sample, so no option set won every workload; the retained
choice favors the high-degree read/build/restore path without adding a second
layout.
