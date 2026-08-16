# Plan 08: Durable Operations and Store Observability

## Outcome

Starting from Plan 06's complete routing-bundle lifecycle, finish the local
operational boundary around the authoritative RocksDB catalog. At completion,
PathHydra has explicit engine shutdown, RocksDB checkpoint backup, validated
offline restore, a read-only inspection/verification tool, structured store and
maintenance metrics, bounded maintenance resources, and workload evidence for
the selected column-family, adjacency, durability, and complete-rebuild choices.

This plan does not rebuild Plan 06's image publication, retirement, cache,
corruption, or out-of-core machinery. It treats those as complete dependencies
and adds database-level operations that the original design still requires.

## Prerequisite

Plan 06 must already provide:

- crash-safe bundle build/pointer publication;
- exact startup reconciliation and corrupt-bundle rebuild;
- bundle leases, safe retirement, and bounded cleanup;
- complete publication failpoints and restart matrix;
- host/device cache shutdown and cancellation;
- bundle lifecycle health and scale benchmarks.

If completed bundles accumulate without retirement, old requests can lose their
bundle, or publication crash points are untested, finish Plan 06 first.

## Explicit non-goals

- making routing bundles authoritative graph backups;
- reimplementing Plan-06 bundle leases, retirement, or publication recovery;
- remote/object-store backup, replication, clustering, or high availability;
- online schema migration, format versions, compatibility readers, graph
  revisions, or caller-pinned historical snapshots;
- revising/rejecting/grouping provisional candidates outside their lifecycle;
- deleting files outside exact validated database/checkpoint roots;
- a general RocksDB administration shell;
- hosted metrics or a required telemetry service;
- an incremental routing overlay hidden inside an operations change.

## 1. Record operational ownership and durability

Add Decision 0010 covering:

- one process owns a database path for mutation and publication;
- confirmed and provisional RocksDB records are authoritative durable state;
- the selected first-release column-family split and key encoding, with the
  section 7 workload evidence that accepts or rejects the current layout;
- the retained one-entry durable adjacency representation and its measured
  operating bounds, without reopening Decision 0008's high-degree routing
  representation;
- routing bundles remain rebuildable Plan-06 indexes;
- checkpoint backups include RocksDB and omit routing bundles by default;
- restore validates every current record/index and rebuilds routing when needed;
- current-record hydration remains intentionally nonhistorical;
- the selected WAL/sync durability behavior for candidate, confirmed, pointer,
  and maintenance writes;
- all operator targets are exact resolved paths beneath caller-selected roots.

Validate database, image, checkpoint, restore, and scratch paths together.
Reject filesystem roots, equal live/backup destinations, unsafe nesting, and any
operation that could overwrite an open database.

## 2. Add explicit engine shutdown

Add idempotent `GraphEngine::shutdown` with a structured report. It:

1. closes new route, mutation, checkpoint, and maintenance admission;
2. signals queued work and handles active work under a bounded policy;
3. invokes the complete Plan-06 CPU I/O and CUDA shutdown paths;
4. flushes RocksDB work required by Decision 0010;
5. releases database/file handles in documented order;
6. requests already-eligible Plan-06 retirement cleanup;
7. returns durations, drained counts, and typed failures.

Drop performs the safe non-reporting fallback but never detaches a worker.
Callers requiring durability assurance invoke `shutdown` explicitly.

## 3. Add RocksDB checkpoints as the backup contract

Expose an engine-coordinated checkpoint operation that:

- validates a new empty destination outside live database/image roots;
- serializes with confirmed mutation sufficiently to capture one consistent
  confirmed/provisional state;
- uses RocksDB's checkpoint facility rather than copying an open directory;
- never changes the active routing image;
- excludes routing bundles by default because they are rebuildable;
- bounds checkpoint concurrency and admits expected disk use/headroom;
- returns records/files/bytes/duration and typed failures.

Candidate records are included because they are durable engine state. Generated
human-readable reports may accompany the checkpoint but are not graph records
or restore authority.

## 4. Add validated offline restore

Restore operates only into a new empty destination and never overwrites an open
or live database. It:

1. validates source/destination paths;
2. copies or opens the checkpoint by the documented RocksDB procedure;
3. runs complete catalog record/name/adjacency/metadata validation;
4. clears an unusable routing pointer if a database-only checkpoint retained it;
5. rebuilds or validates the Plan-06 current bundle;
6. opens a temporary engine and performs read-only smoke checks;
7. returns counts, checksums, durations, and failures.

Do not automatically replace a production directory or delete the source.
Document operator-controlled cutover and rollback.

Test omitted routing files, deliberately corrupt routing files, truncated
checkpoints, malformed records, missing adjacency, candidate preservation, and
restore after confirmed deletion.

## 5. Add a read-only inspection and verification binary

Create `pathhydra-admin` or equivalent with commands for:

- catalog summary counts and resolved resource paths;
- full record/name/adjacency validation;
- active routing pointer and complete Plan-06 bundle validation;
- bundle counts/bytes/partitions/checksums without payload output;
- candidate counts by kind without presenting them as confirmed;
- Plan-06 image-root reconciliation in dry-run mode;
- checkpoint creation and restore validation;
- structured engine health/config snapshot where an in-process open is safe.

Read-only is the default. Mutating maintenance uses explicit subcommands naming
the exact target. Do not implement raw record edits, candidate factual
validation, fuzzy lookup, or arbitrary RocksDB access.

## 6. Expose store and maintenance metrics

Add structured snapshots for:

- write attempts/failures and committed bytes by operation class;
- WAL, sync, flush, and background errors;
- per-column-family live data, table, memtable, and estimated key bytes/counts
  available through documented RocksDB properties;
- block-cache hits/misses/capacity when available;
- pending/running compaction, pending bytes, stalls, and background jobs;
- confirmed scan records/bytes/duration;
- image build/load/validation summaries forwarded from Plan 06;
- active image references and retired storage forwarded from Plan 06;
- checkpoint/restore attempts, failures, bytes, and durations;
- last catalog verification and maintenance outcome;
- shutdown state and drained work.

Unsupported RocksDB properties produce `Unavailable`, not zero. Snapshots need
no global exporter and contain no names, payloads, profiles, destinations,
paths, raw file bytes, or unnecessary absolute paths.

## 7. Measure the current RocksDB layout

Build repeatable workloads for:

- sequential candidate insertion/promotion;
- random node/relation/edge promotion;
- exact-name hit/miss;
- high-degree outgoing/incoming adjacency;
- random edge deletion;
- high-degree cascading node deletion;
- Plan-06 streaming bundle scan/build;
- restart validation;
- checkpoint/restore;
- compaction and space amplification after churn.

Measure the current one-entry adjacency keys and column-family split before
tuning. Report write rate, read latency, amplification, compaction, cache,
database size, scan/build blocking time, and peak memory.

## 8. Select column families and keys, then tune current-use RocksDB options

Use the section 7 evidence to close the original column-family and key-encoding
decision. Retain the current split and keys when no alternative has a
repeatable material benefit across ingest, mutation, deletion, scan/build,
restart, compaction, memory, and space amplification. If an alternative wins
those current workloads, change the one pre-release layout in place and update
fixtures and documentation; do not add a format marker, migration reader, or
compatibility path.

Retain the current one-entry durable adjacency representation and document its
measured bounds. Decision 0008 remains the accepted high-degree routing-image
representation; this operations plan does not reopen it or introduce packed
durable adjacency.

Record and, where evidence supports it, configure write buffers, compression,
block cache, bloom/prefix behavior, background jobs, checksums, WAL, and sync
policy per current column-family access pattern.

Do not add packed adjacency, another transaction model, or extra column
families without measured improvement and complete promotion/deletion/restart
tests. Remove benchmark-only alternatives after the decision.

## 9. Bound maintenance and disk resources

Add configuration/admission for:

- checkpoint concurrency;
- estimated checkpoint bytes and destination headroom;
- restore scratch/headroom;
- maintenance worker count and queue;
- maximum verification duration/work where a caller supplies a limit;
- shutdown drain policy.

Plan-06 routing bundle and retirement limits remain owned by Plan 06. This plan
only exposes their status alongside store limits.

Define typed behavior for disk full during graph write, checkpoint, restore,
flush, and compaction. Never create space by deleting the live database,
current bundle, unknown directories, or an unvalidated target.

## 10. Close complete-rebuild versus overlay publication

Measure confirmed mutation latency and blocked-route duration across graph sizes
and mutation types using the completed Plan-06 pipeline. For the first release,
retain complete immutable bundle rebuilds if they meet documented target
workloads. Record the chosen bounds and the absence of a graph revision API.

If required workloads fail those bounds, stop and create a separate reviewed
incremental/overlay plan. An overlay changes routing, deletion, bundle, GPU,
compaction, and publication semantics; it cannot be smuggled into tuning.

## 11. Add operational recovery rehearsals

Provide local scripts/tests that rehearse:

- clean and abrupt process restart using Plan-06 recovery;
- database-only checkpoint and restore;
- restore followed by routing-bundle regeneration and CUDA reinitialization;
- disk-full/resource refusal;
- failed checkpoint/restore leaving source untouched;
- explicit shutdown during queued and active work;
- validation of a database with provisional and confirmed material;
- operator cutover/rollback between restored directories.

These reuse, rather than duplicate, Plan 06's publication crash matrix.

## 12. Documentation and verification

Update storage, backup, restore, administration, CPU/CUDA operations, and
shutdown documentation. Ensure current reference docs describe Plan-06 startup,
partitioned CUDA, retirement, and recovery accurately.

Required gates include all existing Plan-06 tests plus checkpoint/restore,
inspection, metrics-unavailable, tuning, disk admission, maintenance
concurrency, and shutdown tests. Run ordinary and CUDA feature suites because
shutdown and restore must release/reconstruct both executor modes.

## Definition of done

Plan 08 is complete when:

- Plan 06 remains the sole owner of bundle lifecycle/publication fault behavior;
- explicit shutdown leaves no detached work or ambiguous durability;
- checkpoints preserve complete provisional and confirmed state;
- offline restore validates records/indexes and reconstructs routing safely;
- operators can inspect/verify without raw database mutation;
- RocksDB and maintenance health is structured and distinguishes unavailable
  metrics from zero;
- the first-release column-family and key layout is selected from named
  workload evidence, the accepted one-entry adjacency remains measured and
  documented, and only one current pre-release layout remains;
- current store settings have named workload evidence;
- maintenance/disk resources are bounded and safely refused;
- complete rebuild versus overlay has an explicit measured answer;
- recovery rehearsal and workspace/CUDA gates pass.
