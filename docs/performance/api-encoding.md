# Consumer API encoding evidence

Date: 2026-08-17. Build mode: Rust release. Host: AMD Ryzen 7 9800X3D,
16 logical processors, Windows NT 10.0.26200.0. Toolchain: rustc 1.95.0
(`x86_64-pc-windows-msvc`, LLVM 22.1.2). Selected production encoding: strict
canonical JSON. Binary comparator: Postcard 1.1.3, dev-only.

## Reproduction

```powershell
cargo test -p pathhydra-api --all-features --release --test encoding encoding_candidate_measurements -- --ignored --exact --nocapture
```

The ignored evidence test uses seven samples of 50 iterations after release
compilation and prints machine-readable CSV. Each workload contains the actual
public DTO named below, wrapped only to select a corpus case. It serializes the
same DTO to canonical JSON and Postcard, attempts to decode it, and checks exact
DTO equality. Times are minimum/median/maximum elapsed nanoseconds per operation
from one local run; they are decision evidence, not an API latency promise.
`unsupported` means Postcard returned its typed `WontImplement` error before a
decode could be timed.

| workload | encoding | bytes | encode ns/op min / median / max | decode ns/op min / median / max | correct |
| --- | --- | ---: | ---: | ---: | --- |
| routing-request | JSON | 1,824 | 3,582 / 3,692 / 3,802 | 16,670 / 16,938 / 18,580 | true |
| routing-request | Postcard | 563 | 806 / 806 / 834 | unsupported | false |
| route-path | JSON | 42,071 | 379,030 / 380,492 / 529,770 | 634,232 / 636,700 / 644,654 | true |
| route-path | Postcard | 14,461 | 9,934 / 9,942 / 10,326 | 165,524 / 166,810 / 167,646 | true |
| hydration-response | JSON | 19,000 | 159,106 / 159,752 / 160,318 | 250,306 / 250,830 / 257,828 | true |
| hydration-response | Postcard | 8,404 | 3,526 / 3,536 / 3,698 | unsupported | false |
| health | JSON | 1,941 | 2,346 / 2,354 / 2,422 | 8,788 / 8,828 / 10,996 | true |
| health | Postcard | 211 | 430 / 436 / 494 | unsupported | false |
| large-subgraph | JSON | 679,980 | 10,086,098 / 10,226,252 / 10,351,308 | 17,915,424 / 18,117,438 / 18,770,526 | true |
| large-subgraph | Postcard | 279,984 | 234,374 / 239,970 / 244,158 | 2,774,730 / 2,829,880 / 2,849,830 | true |

The corpus shapes are:

- `routing-request` (`RoutingRequestDto`): maximum origin ID, 64 ordered
  destinations, 16 exact relation multiplier bit patterns, an unlimited budget,
  and the stable-predecessor tie policy;
- `route-path` (`RoutePathDto`): 255 connected `PathStepDto` values preserving
  edge, endpoint, relation-kind, base-weight, multiplier, effective-weight, and
  total-distance evidence;
- `hydration-response` (`HydrationResponseDto`): 128 found `NodeRecordDto`
  values with exact Unicode names and padded opaque payloads;
- `health` (`HealthDto`): lifecycle, image-build, retirement, admission, and CUDA
  health counters and durations;
- `large-subgraph` (`SubgraphHandlesDto`): 10,000 ordered node IDs and 9,999
  ordered directed edge handles.

Postcard is smaller and faster on encode and on the two DTOs it can round-trip.
It cannot decode `RoutingRequestDto`, `HydrationResponseDto`, or `HealthDto`:
their explicit internally tagged enums require serde's self-describing
`deserialize_any` path, for which Postcard 1.1.3 returns `WontImplement`. A
Postcard boundary would therefore require changing the owned public DTO
representation or adding a second schema-specific wire model. JSON is selected
because it round-trips every current DTO and the Rust/BAML application path can
consume and inspect it directly. The JSON decoder's byte/depth/value pre-scan,
mandatory DTO validation, duplicate and unknown field rejection, canonical
wrapper parsing, and typed errors close the correctness and resource-control
gap without adding another production dependency.
