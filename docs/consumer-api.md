# PathHydra consumer Rust API

`pathhydra-api` is the selected in-process application boundary. One
`PathHydra` value owns one durable catalog, its current immutable routing image,
and its CPU/CUDA and partition-I/O workers. Application and BAML code receive
owned DTOs; they do not receive store, routing-image, dense-ID, CUDA, cache, or
worker handles.

The process choice and current-state hydration contract are recorded in
[Decision 0011](decisions/0011-consumer-process-boundary.md). Canonical JSON is
for persistence, caller-owned logging, tests, and later bindings. It does not
start or imply a network service.

## Opening and local resource ownership

For a default local layout:

```rust,no_run
use pathhydra_api::PathHydra;

let api = PathHydra::open("application-data/graph")?;
# Ok::<(), pathhydra_api::ApiError>(())
```

`open` derives private sibling routing-image, checkpoint, restore, and scratch
roots. A deployment that chooses explicit paths uses `PathHydraOpenConfig`:

```rust,no_run
use pathhydra_api::{PathHydra, PathHydraOpenConfig};

let open = PathHydraOpenConfig::new(
    "data/catalog",
    "data/routing-images",
    "data/checkpoints",
    "data/restores",
    "data/scratch",
);
let api = PathHydra::open_with_config(open)?;
# Ok::<(), pathhydra_api::ApiError>(())
```

These are local deployment values, not canonical DTOs. The facade resolves and
validates them together before opening the engine. They must be distinct,
nonnested descendants of existing caller-selected parents. They are never
returned in health, reports, encoded values, or errors.

`PathHydraOpenConfig::with_limits` selects consumer allocation/cardinality
limits. `with_engine_config` accepts the owned, path-free
`PathHydraConfigDto`; `PathHydra::default_engine_config()` supplies its current
default. `capabilities()` reports both effective engine resource limits and
consumer `ApiLimits`. Consumers inspect capability values rather than inferring
CUDA availability from a build feature or device name.

## Finite operation surface

The facade exposes these operation groups; there is no untyped command map or
arbitrary store/image read:

- exact identity: `lookup_node_exact`, `lookup_relation_kind_exact`;
- provisional lifecycle: `insert_node_candidate`,
  `insert_relation_kind_candidate`, `insert_edge_candidate`, `get_candidate`,
  `confirm_candidate`;
- confirmed records and deletion: `get_confirmed_node`,
  `get_confirmed_relation_kind`, `get_confirmed_edge`,
  `remove_confirmed_edge`, `remove_confirmed_node`;
- routing and cancellation: `request_handle`, `cancel`,
  `RequestHandle::route`, `RequestHandle::cancel`;
- current-state hydration: `hydrate`, `RequestHandle::hydrate_path`,
  `hydrate_subgraph`;
- caller-owned subgraphs: `new_subgraph`, `subgraph_add_node`,
  `subgraph_add_edge`, `subgraph_remove_node`, `subgraph_remove_edge`,
  `subgraph_union`, and `RequestHandle::add_path_to_subgraph`;
- runtime inspection/rebuild: `capabilities`, `health`,
  `maintenance_status`, `rebuild_routing`, `rebuild_cuda_residency`,
  `reinitialize_cuda`;
- durable operations: `verify_catalog`, `create_checkpoint`,
  `validate_checkpoint`, `compact_store`, `restore_checkpoint`;
- lifecycle: `shutdown`.

Candidate insertion stores provisional material only. It is excluded from exact
confirmed lookup, routing, and hydration until the caller makes an external
decision and calls `confirm_candidate`. Confirmation accepts no confidence
score and does not claim to validate facts. `MutationOutcomeDto` reports the
durable confirmed record or removed stable ID separately from routing-image
publication outcome.

Confirming a second node or relation-kind candidate with an already confirmed
exact name consumes that candidate and returns the existing confirmed record
with its original stable ID. It never creates a second confirmed identity; the
same result also reports the routing publication outcome.

Exact names are case-sensitive strings. The facade does not normalize, fold,
correct, alias, fuzzy-match, or merge them. Opaque payload bytes use
`PayloadDto`; payload content is never placed in an error.

## Request identity and cancellation

Allocate an owned handle, then execute its one route attempt:

```rust,no_run
# use pathhydra_api::{PathHydra, RequestIdAllocation, RoutingRequestDto};
# fn route(api: &PathHydra, request: &RoutingRequestDto) -> Result<(), pathhydra_api::ApiError> {
let handle = api.request_handle(RequestIdAllocation::Automatic)?;
let cancellation = handle.clone();
let response = handle.route(request)?;
println!("request {} completed", cancellation.id());
# let _ = response;
# Ok(()) }
```

The facade allocator is monotonic and checked. An automatic ID follows the
current process high-water mark. `RequestIdAllocation::Supplied` accepts an
application correlation ID only when it is strictly greater than every ID
already assigned, then advances the high-water mark. Lower, duplicate, and
reused values are rejected before engine admission. `u64::MAX` can be assigned
once; every later allocation returns `request_id_exhausted`. Request IDs are
process-local coordination values, never graph identity, and are invalid after
restart.

Cloned handles refer to the same request, flag, and response. The route method
can execute once. Cancellation is repeatable and returns
`CancellationOutcomeDto`:

- `signalled`: the shared flag transitioned to signalled. Before admission it
  is registered already signalled; while queued or running it is the same flag
  observed by CPU/CUDA execution;
- `already_signalled`: another cancellation already signalled it;
- `already_completed`: the complete response won the race and cannot be
  retroactively changed;
- `unknown_request`: no live handle exists for the never-reusable ID;
- `shutting_down`: shutdown prevents a not-yet-admitted handle from executing,
  or an unknown cancellation arrives after admission closes.

Routing is synchronous and returns one complete `EngineRoutingResponseDto`.
`CompletionReasonDto::Cancelled` is a response completion, not necessarily an
API call error. Destination states keep exact, unreachable, missing, and
incomplete distinct. Exact binary32 operands and binary64 distance evidence are
stored as fixed bit text by their DTO wrappers.

## Hydration and subgraphs

A completed request handle retains its point-in-time routing response so the
caller can hydrate a selected path or add that exact structural path to a
subgraph. Routing evidence remains point-in-time; hydration always resolves
current confirmed records. If a returned node or edge was deleted after the
route, `hydrate_path` returns typed `HydrationUnavailable` rather than stale
records. Arbitrary `hydrate` and `hydrate_subgraph` preserve explicit missing
states and IDs.

`SubgraphHandlesDto` is a deterministic caller-owned value: node IDs and edge
IDs are strictly increasing, and every directed edge carries source and
destination evidence whose endpoints must be present. Subgraph edits never
mutate provisional or confirmed database state. Union validates every edge ID
for endpoint conflicts before changing the destination subgraph.

Encode and decode a handle-only subgraph with the same configured limits:

```rust,no_run
# use pathhydra_api::{decode, encode, PathHydra, SubgraphHandlesDto};
# fn round_trip(api: &PathHydra, subgraph: &SubgraphHandlesDto) -> Result<(), Box<dyn std::error::Error>> {
let bytes = encode(subgraph, &api.limits())?;
let decoded: SubgraphHandlesDto = decode(&bytes, &api.limits())?;
assert_eq!(&decoded, subgraph);
# Ok(()) }
```

Handle-only and hydrated subgraphs are separate DTOs. Hydrated records are
outputs and do not become authoritative confirmed-mutation inputs.

## Checkpoint, restore, maintenance, and shutdown

Canonical maintenance commands use opaque single-component names such as
`nightly-2026-08-17`, not paths. `create_checkpoint` resolves a name beneath
the configured checkpoint root. `validate_checkpoint` opens that immutable
checkpoint read-only. `restore_checkpoint` resolves one source name and one
fresh destination name beneath the configured checkpoint/restore roots,
rebuilds routing at a private derived root, runs catalog/route/hydration smoke
checks, initializes configured CUDA when enabled, and shuts down the temporary
engine. Reports contain counts, bit-exact checksums, durations, outcomes, and
safe categories only—never resolved paths.

`verify_catalog` and `compact_store` use the engine's bounded maintenance
lifecycle. Concurrent maintenance can return `maintenance_busy`; work racing
shutdown either finishes within the drain or receives a typed shutdown error.

Call `shutdown` when its report and explicit durability boundary matter. It
closes admission, signals and drains active work, joins owned workers, flushes
the store, releases its handle, and returns per-stage safe failures. It is
idempotent. Dropping the final owner performs best-effort shutdown for unwind
safety, but cannot return an operational report.

## Errors and safe diagnostics

`ApiError` carries a stable `ApiErrorCategory`, stable `code`, static safe
`message`, and `retryable` flag. It never retains or formats an internal error,
payload, exact name, device diagnostic text, or arbitrary local path. Categories
distinguish invalid input/encoding/profile/weight, missing candidate/confirmed
records, name and subgraph conflicts, durable store/corruption/recovery,
routing/image availability, admission/resource limits, cancellation,
hydration/integrity, CUDA availability/ineligibility/device failure,
backup/restore/maintenance, shutdown, and internal invariants.

Incomplete routing remains a response state. It is not flattened into an API
error. Health likewise uses structured booleans, enums, counts, and durations;
unsafe internal reason strings are reduced to safe presence/category fields.

## Executable integration harness

The complete non-BAML example inserts and confirms a small exact graph, routes
exact/unreachable/missing destinations, hydrates a path, round-trips a
subgraph, reads health, and shuts down:

```text
cargo run -p pathhydra-api --example consumer
```

The application remains responsible for external validation and for deciding
which paths contribute to any final graph composition.
