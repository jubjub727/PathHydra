# Local administration

Relation popularity is authoritative current catalog metadata, not a runtime
query statistic. `most_used_relation_kinds` performs a bounded ordered snapshot
read. Verification, checkpoint validation, and restore independently reconcile
both usage domains and the popularity index. Administrators must not edit
candidate, edge, adjacency, relation usage, or popularity records directly;
all graph mutations go through catalog operations under the mutation lock.

`pathhydra-admin` is a finite local administration tool, not a RocksDB shell.
Inspection opens an existing catalog through the strict read-only store API and
never initializes a missing directory. Successful output is one compact JSON
document containing aggregates only: no exact names, payloads, raw records,
profiles, routing destinations, or dense runtime identities. The local-operator
`summary` command is the deliberate exception for filesystem paths: it echoes
the exact resolved catalog path so an operator can verify the inspected target.
Metrics and engine-health snapshots remain path-free.

Build or run it as the workspace administration binary:

```powershell
cargo build --release -p pathhydra-admin
cargo run --release -p pathhydra-admin -- help
```

## Read-only commands

```text
summary --database PATH
candidate-counts --database PATH
verify --database PATH [--max-records N] [--max-duration-ms N]
active-pointer --database PATH [--routing-root PATH]
metrics-snapshot --database PATH
reconcile-routing-root-dry-run --database PATH --routing-root PATH
```

`summary` reports the exact resolved database path plus confirmed, provisional,
index, adjacency, and pointer counts.
Candidate counts are explicitly separate from confirmed counts. `verify`
decodes and cross-checks all current records and indexes; its record and
duration limits turn maintenance budgets into typed refusals.

`active-pointer` reports pointer presence and checksum. With a routing root, it
opens the exact safe child through the production bundle reader and reports
checksum agreement, aggregate bytes, partitions, and segments. It never prints
the child name. `reconcile-routing-root-dry-run` recognizes only `bundle-*` and
`.tmp-*` children, validates bundles, counts what Plan 06 could clean, retains
unknown entries, and always reports `"mutated":false`.

`metrics-snapshot` distinguishes an unavailable RocksDB property with JSON
`null`; it never substitutes zero. A freshly opened read-only inspector has no
historical in-process catalog write counters, so those counters begin at zero
while RocksDB properties describe the opened database. Offline restores are
process-owned rather than catalog-owned; their aggregate attempts, failures,
bytes, and duration remain visible in `standalone_restore` for the lifetime of
the administration/engine process.

## Explicit destination-writing commands

```text
checkpoint-create --database PATH --destination-root ROOT --destination PATH
  [--routing-root PATH] [--scratch PATH]
  --available-bytes N [--headroom-bytes N]

restore-validate --source-root ROOT --source PATH
  --destination-root ROOT --destination PATH
  --routing-root PATH [--scratch PATH]
  --available-bytes N [--headroom-bytes N]
  [--max-records N] [--max-duration-ms N]
```

These commands write only the exact named fresh destination. They do not replace
a live database, delete a source, or perform cutover. Coordinate checkpoint with
the owning engine. Restore is offline and opens a temporary production engine,
rebuilds the routing bundle, runs route and hydration smoke checks, initializes
CUDA when configured by the caller, and requires a complete explicit shutdown.

`engine-health --database PATH --routing-root PATH` is an explicitly named
offline engine command. It may validate or rebuild the Plan 06 bundle, emits
only redacted capability/resource aggregates, and explicitly shuts down before
returning. Its shutdown aggregate includes active-before and drained counts for
routes, mutations, checkpoints, and maintenance. Do not point it at a catalog
owned by another process.

Engine verification always applies the configured record-work and duration
ceilings. An omitted or larger caller limit is reduced to those ceilings rather
than silently making verification unbounded.

## Workload evidence

```text
workload --root FRESH_PATH [--scale N] [--samples N]
```

The workload command is explicitly mutating and refuses a nonempty root. It
generates only synthetic names/payloads beneath that root, validates each
operation, and emits a machine-readable JSON report with raw nanosecond samples,
median, p95, byte counts, peak working-set bytes where the platform exposes
them, and final catalog checksum. It never changes a caller's live catalog. See
[RocksDB operations performance](performance/rocksdb-operations.md).

## Errors and safe use

Exit code 2 indicates a command/argument error; exit code 1 indicates an
operation or validation failure. Errors intentionally avoid echoing graph
names, payloads, records, or target paths. Keep the underlying store/engine
structured report in an access-controlled operator channel when deeper
diagnosis is required.

Only one process may own a live catalog for mutation. Read-only inspection can
run where the platform and RocksDB lock behavior allow it, but full verification
is intentionally expensive. Supply explicit limits in automation.
