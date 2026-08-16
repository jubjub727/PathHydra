# Consumer API encoding evidence

Date: 2026-08-17. Build mode: Rust release. Host: AMD Ryzen 7 9800X3D,
16 logical processors, Windows NT 10.0.26200.0. Toolchain: rustc 1.95.0
(`x86_64-pc-windows-msvc`, LLVM 22.1.2). Selected production encoding: strict
canonical JSON. Binary comparator: Postcard 1.1.3, dev-only.

## Reproduction

```powershell
cargo test -p pathhydra-api --release --test encoding encoding_candidate_measurements -- --ignored --nocapture
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
| routing-request | JSON | 1,824 | 3,744 / 3,768 / 4,160 | 17,014 / 17,526 / 21,558 | true |
| routing-request | Postcard | 563 | 880 / 882 / 934 | unsupported | false |
| route-path | JSON | 42,071 | 392,042 / 405,536 / 490,036 | 668,884 / 687,704 / 760,532 | true |
| route-path | Postcard | 14,461 | 9,934 / 9,960 / 10,292 | 169,418 / 172,788 / 178,712 | true |
| hydration-response | JSON | 18,807 | 158,456 / 161,656 / 242,488 | 249,960 / 256,226 / 260,876 | true |
| hydration-response | Postcard | 8,385 | 3,302 / 3,316 / 4,342 | unsupported | false |
| health | JSON | 1,941 | 2,286 / 2,288 / 2,310 | 9,164 / 9,360 / 12,718 | true |
| health | Postcard | 211 | 430 / 432 / 488 | unsupported | false |
| large-subgraph | JSON | 679,980 | 11,884,702 / 12,166,512 / 12,508,110 | 18,636,218 / 18,858,926 / 20,288,900 | true |
| large-subgraph | Postcard | 279,984 | 236,622 / 249,806 / 272,134 | 2,898,306 / 2,947,932 / 3,056,798 | true |

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
