# Backup and restore

Checkpoints include provisional mixed batches, stable candidate-to-candidate
references, relation-kind usage counts, and the complete popularity index.
Validation and restore recompute dependency, provisional-use, and
confirmed-edge counts from candidates and canonical edges; a missing, extra,
stale, or malformed popularity entry is corruption. A restored unconfirmed
mixed batch can be confirmed with the same dependency-complete candidate-ID
selection. Routing bundles remain rebuildable and optional.

RocksDB checkpoint is PathHydra's local backup contract. Confirmed records and
provisional candidates are authoritative and are included. Routing-image
bundles are rebuildable and are omitted by default.

## Create a checkpoint

Stop or coordinate graph mutation through the owning engine before using the
standalone command. Name the live database and the exact fresh destination:

```powershell
cargo run --release --manifest-path crates/pathhydra-admin/Cargo.toml -- `
  checkpoint-create `
  --database D:\PathHydraData\live\catalog `
  --destination-root D:\PathHydraBackups `
  --destination D:\PathHydraBackups\checkpoint-2026-08-17 `
  --available-bytes 1099511627776 `
  --headroom-bytes 10737418240
```

The destination must be absent or empty, beneath the supplied root, outside the
live database and routing-image roots, and admitted for the reported database
bytes plus headroom. The command uses RocksDB checkpoint; it does not copy an
open directory. Its JSON report contains only counts, bytes, checksums, and
durations.

Checkpoint concurrency is bounded by `CatalogConfig`. A disk or concurrency
refusal leaves the live database and destination source untouched. Never make
space by deleting the live catalog, current routing bundle, or unknown files.

## Validate an offline restore

Restore always targets a new empty directory and never replaces a live path:

```powershell
cargo run --release --manifest-path crates/pathhydra-admin/Cargo.toml -- `
  restore-validate `
  --source-root D:\PathHydraBackups `
  --source D:\PathHydraBackups\checkpoint-2026-08-17 `
  --destination-root D:\PathHydraRestore `
  --destination D:\PathHydraRestore\catalog-validated `
  --routing-root D:\PathHydraRestore\routing-validated `
  --available-bytes 1099511627776 `
  --headroom-bytes 10737418240 `
  --max-records 100000000 `
  --max-duration-ms 3600000
```

Validation checks metadata, candidates, confirmed records, exact-name indexes,
both adjacency directions, embedded IDs, counters, and a stable operational
checksum. A database-only checkpoint may retain an old routing pointer; restore
clears it when its bundle is unavailable. The command opens a temporary
production engine, rebuilds a current Plan 06 bundle, performs exact route and
hydration smoke checks, initializes CUDA when requested by the engine
configuration, and requires explicit shutdown before reporting success.

Failure never modifies the checkpoint source. A partially created fresh
destination is not a valid restore and must not be selected for cutover. Keep it
for diagnosis or remove that exact operator-selected destination only after
confirming it is not live.

## Cutover and rollback

PathHydra does not automatically switch production directories.

1. Explicitly shut down the current engine and retain its structured report.
2. Run `summary` and `verify` against the validated restore.
3. Open a temporary engine on the restore, rebuild the routing image, run
   read-only routing/hydration smoke checks, and initialize CUDA if required.
4. Change the service configuration to the validated destination using the
   deployment system's atomic configuration mechanism.
5. Start one owner and verify health before accepting work.
6. Keep the old directory unchanged as the rollback target.

Rollback repeats shutdown, changes configuration back to the retained old
directory, and starts one owner. Do not merge writes made independently after
cutover; there is no graph revision or replication protocol.

Use [the recovery rehearsal](../Scripts/rehearse-operations.ps1) on local data
before relying on these steps.

The repository-owned rehearsal covers synthetic confirmed and provisional
state, checkpoint, zero-byte disk refusal, validated engine restore, a refused
nonempty restore preserving source and marker, abrupt Plan 06 restart, active
shutdown timeout/retry, routing/hydration reconstruction, optional real-device
CUDA reconstruction, and cutover/rollback aggregate equivalence. Run it with
`-Cuda` on the supported device; missing required checks are failures rather
than optional callback warnings.
