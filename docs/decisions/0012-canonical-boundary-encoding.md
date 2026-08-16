# Decision 0012: Canonical boundary encoding

Status: accepted

PathHydra selects strict canonical JSON for owned consumer DTOs. This encoding
supports persistence, caller-controlled logging, test fixtures, and later
language bindings; it does not create a transport protocol or hosted service.
There is one current pre-release representation and no schema/version marker,
compatibility reader, or negotiation envelope.

## Candidates and evidence

The release-comparable corpus covers a multi-destination routing request, exact
path evidence, hydrated records with opaque payloads, a structured health
report, and a 10,000-node/9,999-edge handle subgraph. The executable command,
raw measurement method, sizes, and encode/decode distributions are recorded in
`docs/performance/api-encoding.md`. Every timed decode is checked for exact DTO
equality, and unsupported candidate decodes are recorded rather than excluded.

Postcard is materially smaller and faster on encode and on the public DTOs it
can round-trip. It is nevertheless not selected. On the actual owned corpus,
Postcard 1.1.3 returns `WontImplement` while decoding `RoutingRequestDto`,
`HydrationResponseDto`, and `HealthDto`: those DTOs contain explicit internally
tagged enums that require serde's self-describing `deserialize_any` path. A
Postcard boundary would therefore require changing the public representation or
maintaining a second Rust-specific schema-aware wire model. The current
consumer includes BAML and application code for which JSON has direct
inspectability and ubiquitous tooling, and no current deployment or transport
user offsets that correctness and integration cost. The bounded canonical JSON
implementation remains small and directly auditable. Postcard stays a
benchmark-only development dependency.

## Canonical JSON contract

- Output is UTF-8 without a byte-order mark, insignificant whitespace, or a
  trailing document. Struct declaration order is field order. Boundary DTOs do
  not expose nondeterministically iterated maps.
- Strings use `serde_json`'s deterministic compact escaping. Unicode scalar
  sequences, case, punctuation, and compatibility characters remain exact;
  PathHydra performs no normalization.
- Stable node, relation-kind, edge, candidate, and process-local request IDs
  are minimal unsigned decimal strings. `u64::MAX` therefore never crosses a
  JavaScript number boundary.
- Binary32 and binary64 evidence is `0x` followed by exactly 8 or 16 lowercase
  hexadecimal digits. Contextual validation rejects NaN, infinity, negative
  values, negative zero, or out-of-range values where graph semantics forbid
  them.
- Opaque payloads use the RFC 4648 standard base64 alphabet with canonical
  padding. A decoder rejects alternate alphabets, omitted/noncanonical padding,
  and unused-bit aliases.
- Enums use explicit snake-case names. Semantically distinct absent, disabled,
  missing, unreachable, incomplete, and invalid states remain distinct.
- Ordered arrays carry profiles, destinations, paths, hydration handles, and
  subgraphs. Subgraph node and edge IDs are strictly increasing and unique;
  every endpoint must be present. Parallel and self edges remain distinct by
  stable edge ID.
- The ordinary decoder may accept field reordering and insignificant
  whitespace. Decode/re-encode always returns the one canonical byte sequence;
  the strict decoder rejects bytes differing from that sequence.

## Decoder and allocation rules

`ApiLimits` bounds encoded bytes, names, payloads, destinations, profile
entries, path steps, hydration handles, subgraph nodes/edges, diagnostic text,
nesting depth, and aggregate JSON values. Before serde deserialization, a
no-allocation scan rejects oversized bytes, invalid UTF-8, a BOM, unmatched
containers, excessive depth, and excessive aggregate values. DTO derives reject
unknown and duplicate fields. One deserializer followed by `end()` rejects
truncation and trailing documents.

Every decodable boundary DTO implements `CanonicalDto::validate_boundary`.
The default exported decoder always invokes it, so field-specific limits,
profile ordering, numeric evidence, path continuity, duration bounds, and
subgraph integrity cannot be bypassed. Validation completes before any facade
mutation. Encoding uses a hard-capped writer with small initial capacity and
fallible incremental reservation. Allocation failure, limit refusal, and
malformed/context-invalid input are separate typed errors. Error display never
echoes input bytes, payloads, exact names, or paths.

## Dependencies and licensing

The locked production dependencies are `serde` 1.0.229, `serde_json` 1.0.151,
and `base64` 0.22.1. The measured binary candidate is dev-only `postcard`
1.1.3. Each declares `MIT OR Apache-2.0`; no additional runtime dependency is
selected. Semantic validation remains in PathHydra rather than serializer
extensions.
