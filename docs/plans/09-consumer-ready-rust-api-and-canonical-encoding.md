# Plan 09: Consumer-Ready Rust API and Canonical Encoding

## Outcome

Finish the narrow typed Rust boundary consumed above the graph engine. At
completion, an application can use one coherent Rust facade for candidate
lifecycle, exact lookup, confirmed deletion, routing, cancellation, hydration,
subgraph operations, capabilities, health, backup/maintenance, and shutdown.
Every value that must cross an application boundary has one bounded canonical
encoding, including subgraphs and exact numeric evidence.

This plan does not define BAML prompts, models, workflows, or how a caller
chooses paths for a final graph. It provides the complete deterministic Rust
surface those components consume.

## Prerequisite and remaining boundary

Plans 06-08 must be complete. The facade consumes their final resident and
partitioned CPU/CUDA capabilities, bundle lifecycle, backup/restore,
maintenance, health, shutdown, and error shapes; it does not compensate for a
partial earlier plan.

After those plans, the remaining boundary is consumer packaging and encoding.
The engine operations exist across several crates, but the public shapes are
implementation-oriented and pre-release. Request IDs require low-level
coordination, and there is no canonical request, response, hydration, health,
or subgraph encoding. `SubgraphHandles` chooses no serialized format.

## Explicit non-goals

- a general graph query language;
- a rule language after routing;
- BAML prompt/workflow/model design;
- external factual validation of candidates;
- HTTP/gRPC/WebSocket, authentication, tenancy, or remote hosting;
- direct RocksDB or routing-image access from the consumer;
- schema-version markers, compatibility readers, migrations, graph revisions,
  or caller-pinned historical images before the first release;
- exposing dense node IDs, raw hashes, device pointers, file paths, or cache
  handles;
- normalizing, correcting, aliasing, or fuzzy-matching names.

## 1. Select and record the process boundary

Begin with the process-model decision deliberately left open by the original
design. Compare an in-process Rust library with a separately hosted local Rust
API against the current consumer and deployment requirements. Record at least:

- call and serialization cost for representative routing, hydration, and
  subgraph results;
- deployment complexity and whether a separately managed process has a current
  user;
- failure isolation, shutdown, cancellation, and recovery behavior;
- access control needed to keep RocksDB and routing images solely owned by the
  Rust layer;
- target-platform and BAML integration constraints;
- the effect on typed errors, request identity, and large bounded payloads.

Prefer the smaller in-process boundary when the hosted candidate has no
measured current benefit, consistent with the prohibition on abstractions
without a current use. If the hosted candidate wins materially, stop and write
a separate reviewed transport plan; HTTP, gRPC, authentication, and remote
deployment are not hidden inside this facade plan.

Add Decision 0011 recording the evidence and selected boundary. If the
in-process boundary is selected, record:

- one facade owns `GraphEngine` and its worker lifecycle;
- calls are synchronous unless they explicitly return a request handle for
  cancellation/coordination;
- routing responses are returned as complete structured values; no partial
  streaming protocol is required for correctness;
- application/BAML code may run in the same process and receives no store/image
  handles;
- canonical encoding supports persistence, logging-by-the-caller, testing, and
  later bindings but does not imply a network service;
- a hosted transport, if later required, must adapt these DTOs rather than
  changing graph semantics.

Regardless of packaging, also record the current-state hydration decision:
routing evidence is point-in-time, while hydration intentionally resolves
current confirmed records and reports every missing handle after deletion. No historical snapshot
retention or pinned image API is added.

## 2. Add a dedicated consumer facade crate

For the selected in-process boundary, add one real workspace crate, for example:

```text
crates/pathhydra-api/
  src/
    lib.rs
    facade.rs
    command.rs
    dto.rs
    codec.rs
    error.rs
    limits.rs
  tests/
    lifecycle.rs
    encoding.rs
    malformed.rs
    concurrency.rs
```

The facade delegates to `GraphEngine`; it does not duplicate storage, routing,
hydration, or subgraph logic. Keep low-level crates available for internal and
specialized Rust use, but make this crate the documented application boundary.

## 3. Define a finite command surface

Provide typed calls equivalent to:

- resolve exact node and relation-kind names;
- insert node, relation-kind, and directed-edge candidates;
- retrieve a candidate for external review;
- confirm one externally validated candidate;
- get confirmed node, relation kind, and edge records;
- remove a confirmed edge;
- remove a confirmed node and incident edges;
- route one origin to ordered destinations;
- cancel a request by process-local request ID;
- hydrate arbitrary stable handles;
- hydrate one returned path;
- create/edit/union/encode/hydrate a caller-owned subgraph;
- inspect capabilities and health;
- rebuild routing/CUDA state;
- create/validate checkpoints and inspect maintenance status;
- shut down explicitly.

Do not expose arbitrary column-family reads, dense IDs, routing partition reads,
kernel selection internals, or an untyped command map.

## 4. Make request identity safe and ergonomic

Retain `RequestId` as process-local coordination, never graph identity. Add a
facade-owned monotonic allocator with checked exhaustion and an option for a
caller-supplied ID when correlation is required. Reject duplicates before
admission. Return a handle containing the assigned ID and cancellation method
without borrowing the caller's request.

Define cancellation races precisely:

- before admission;
- queued;
- running CPU/CUDA;
- response already complete;
- unknown/reused ID;
- shutdown.

Encoded request IDs are diagnostic/correlation values and are not valid after
process restart.

## 5. Define canonical boundary DTOs

Create owned DTOs independent of internal locks, `Arc`, iterators, RocksDB,
CUDA, and filesystem types. Cover:

- all stable IDs and exact names;
- opaque payload bytes;
- candidate and confirmed records;
- relation profiles and enabled/disabled state;
- routing request, budget, policies, destination states, exact path evidence,
  and completion;
- executor/runtime diagnostics;
- hydration request/response and unavailable evidence;
- subgraph handles and hydrated subgraph;
- mutation/publication outcomes;
- capabilities, health, backup, restore, maintenance, and shutdown reports;
- typed errors with stable machine-readable categories and safe messages.

Internal type-to-DTO conversion revalidates checked numeric fields where needed
and cannot panic on public input.

## 6. Select a canonical encoding and encode evidence losslessly

Evaluate the encoding decision deliberately left open by the original design.
Compare strict canonical JSON with at least one compact binary candidate on the
current Rust/BAML consumer path. Record losslessness, deterministic-byte
support, bounded-decoder behavior, malformed-input handling, implementation
and tooling cost, encoded size, and encode/decode time for representative
requests, paths, hydrated records, health reports, and large subgraphs.

Select one encoding in Decision 0012. Prefer strict canonical JSON only if it
meets the recorded limits without custom behavior that is harder to audit than
the binary candidate. The remainder of this section specifies the JSON
candidate; if another encoding wins, replace these candidate-specific rules in
place with equally precise canonical rules before implementing the codec. The
selected current pre-release encoding has no schema/version marker and is
updated in place before release.

For canonical JSON, rules include:

- UTF-8 output, no byte-order mark, no insignificant whitespace, one fixed
  object-field order, and one documented escaping policy;
- the encoder always emits one byte representation for a DTO; decoder
  acceptance of noncanonical field order or whitespace, if intentionally
  supported, is distinct from canonical encoder output and decode/re-encode
  always produces the canonical bytes;
- `NodeId`, `RelationId`, `EdgeId`, `CandidateId`, and request IDs encode as
  canonical decimal strings so JavaScript-style number limits cannot truncate
  them;
- canonical binary32 base weights/multipliers include exact `u32` bits in fixed
  hexadecimal text;
- logical/effective binary64 distances include exact `u64` bits in fixed
  hexadecimal text;
- human-readable decimal renderings are derived display fields outside the
  canonical encoded DTO, so their presence, spelling, or formatting cannot
  create multiple canonical byte representations;
- opaque payload bytes use one canonical base64 alphabet/padding rule;
- exact names remain JSON strings with no normalization or case folding;
- enums use explicit names rather than implicit numeric ordinals;
- absent, disabled, missing, unreachable, incomplete, and invalid remain
  distinct;
- maps whose ordering could vary encode as deterministically ordered arrays;
- no field contains dense node IDs, raw lookup hashes, local paths, or secrets.

Use a maintained serializer/base64 dependency only for the selected immediate
production use, record versions/licenses, and keep semantic validation in
PathHydra.

## 7. Bound every decoder before allocation

Add `ApiLimits` for encoded bytes, name bytes, payload bytes, destination count,
profile entries, path steps, hydration handles, subgraph nodes/edges, diagnostic
text, and nesting depth where the parser supports it. For the JSON candidate,
reject duplicate object fields, trailing documents, invalid UTF-8,
noncanonical ID text, noncanonical hex/base64, unknown enum values, invalid
floats, duplicate profile entries, and endpoint conflicts. If another encoding
is selected, replace these syntax-specific checks with equivalent canonical and
malformed-input rules for that encoding.

Parsing does not mutate engine state. Mutation commands are converted and fully
validated before invoking the engine. Allocation failure and limit refusal are
typed separately from malformed input.

## 8. Make subgraph encoding round-trip structural evidence

Encode ordered node IDs and ordered edge handles containing `EdgeId`, source,
and destination. Decode into a temporary structure, validate:

- strictly increasing unique node IDs;
- strictly increasing unique edge IDs;
- both edge endpoints present;
- no edge ID associated with conflicting endpoints;
- all counts/limits.

Only then construct the caller-owned `Subgraph`. Round trips preserve exact
stable handles and deterministic order. Encoding or decoding never hydrates or
mutates confirmed/provisional state.

Provide separate encodings for handle-only and hydrated subgraphs. Hydrated
records do not become authoritative inputs for confirmed mutation.

## 9. Preserve provisional/confirmed lifecycle distinctions

Command and result DTOs use the words candidate, confirmed node, relation kind,
and edge precisely. A confirmation command represents an external decision
already made; it does not accept a confidence score or pretend to validate
facts. Encoded candidate records cannot be passed where confirmed records are
required without explicit confirmation.

Duplicate exact-name confirmation preserves the existing stable confirmed ID
and reports the consumed candidate/publication outcome without creating a
second confirmed identity.

## 10. Define public error taxonomy

Map internal errors without flattening them into strings. Categories include:

- invalid input/encoding/profile/weight;
- missing candidate/confirmed handle;
- invalid candidate transition;
- exact-name conflict;
- durable store/corruption/recovery;
- routing unavailable/image corruption;
- admission/resource limit;
- cancellation;
- incomplete routing result (which remains a response state, not necessarily an
  API call error);
- hydration unavailable/integrity;
- subgraph conflict;
- CUDA unavailable/ineligible/device failure;
- backup/restore/maintenance;
- shutdown/internal invariant.

Messages never expose payload contents or arbitrary local paths. Preserve enough
structured fields for a caller to decide whether retry, repair, or user input is
appropriate.

## 11. Make capabilities self-describing

Capabilities state the actual supported request shapes and modes from Plans 07
and 08: resident/partitioned CPU/CUDA, path evidence mode, finite-budget policy,
durable bundles, current-state hydration, subgraph encoding, backup/restore,
cancellation, and all configured limits. Health encoding includes only
structured safe fields.

Consumer code must inspect capabilities rather than infer CUDA support from a
build feature or device name.

## 12. Provide a minimal consumer integration harness

Add an example Rust application showing the fixed boundary without defining a
BAML workflow:

1. open the facade;
2. insert and confirm a small exact graph after a placeholder external decision;
3. resolve exact names;
4. submit a multi-destination context profile;
5. inspect exact/missing/unreachable/incomplete states;
6. hydrate one path;
7. add selected paths to a caller-owned subgraph;
8. encode and decode the subgraph;
9. read health and shut down.

The example may be called from the same application code that invokes BAML, but
PathHydra does not import prompts or assume how results are used.

## 13. Compatibility and API cleanup

This remains pre-release. Maintain one current API/encoding, update fixtures in
place, and remove superseded fields/converters rather than adding legacy
adapters. Do not add `v1` envelopes, version negotiation, deprecated aliases, or
migration readers.

Audit every public type for:

- precise terminology;
- constructors that enforce invariants;
- read-only access where mutation would violate ownership;
- `must_use`, `Error`, `Send`, and `Sync` behavior where intended;
- allocation/overflow errors rather than panics;
- no leaked implementation types;
- complete rustdoc examples.

## 14. Required tests

Cover:

- recorded process-boundary and encoding-candidate evidence using named
  representative workloads;
- canonical byte-for-byte encodings and decode/re-encode equality;
- maximum `u64` IDs without target-consumer truncation;
- exact float-bit encoding and separation from derived decimal display;
- Unicode-sequence/case/punctuation name distinctions;
- arbitrary payload bytes and payload limits;
- every candidate and confirmed record shape;
- all routing states, duplicates, policies, paths, and diagnostics;
- current-state hydration after deletion;
- subgraph round trip, parallel/self edges, and endpoint conflicts;
- malformed/truncated/duplicate/trailing/oversized selected encodings,
  including duplicate JSON object fields if JSON is selected;
- all error categories without secret/path leakage;
- concurrent request ID allocation/cancellation;
- shutdown and maintenance command races;
- a full encoded lifecycle through the consumer facade.

## Definition of done

Plan 09 is complete when:

- the original process-model and encoding decisions are selected from recorded
  current workload, target-platform, and consumer evidence;
- one documented selected Rust facade covers every original public API
  operation;
- all boundary values have bounded owned DTOs and canonical lossless encoding;
- subgraphs round-trip exact structural handles;
- stable IDs and numeric evidence cannot be truncated by selected boundary
  consumers;
- current-state hydration and executor specialization are explicit;
- consumer errors are structured and safe;
- no store/image/CUDA implementation handle crosses the facade;
- the integration harness demonstrates the complete boundary without defining
  BAML internals;
- malformed, limit, lifecycle, concurrency, and round-trip tests pass;
- superseded pre-release API shapes are removed rather than preserved by
  compatibility code.
