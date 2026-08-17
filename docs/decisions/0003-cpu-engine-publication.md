# Decision 0003: CPU engine publication ownership

## Status

Accepted.

## Decision

`GraphEngine` exclusively owns one `Catalog` and one published routing state. It does not expose either a mutable catalog reference or a replaceable image. Provisional insertion never rebuilds the image. Promotion, edge removal, and cascading node removal compile, validate, and publish one complete current bundle before another route can acquire a new image.

The state is an available immutable execution image/bundle lease with publication metadata or a typed unavailable reason. Opening validates the catalog, then follows the selected startup policy: the default validates and reuses the exact referenced bundle and rebuilds only when the pointer/bundle is absent or unusable; `RequireValidBundle` reports routing unavailable instead of rebuilding. A valid catalog remains available for inspection, provisional insertion, repair mutations, health, and explicit rebuild even when routing is unavailable.

A confirmed mutation holds the publication write lock, commits through the catalog, performs a consistent streaming confirmed scan into a complete durable bundle, validates it, then publishes the replacement with one assignment. A pre-commit store failure preserves the current image. A post-commit build failure unpublishes the stale image and returns a committed mutation with an explicit unavailable publication outcome. It never reports the durable operation as failed and never republishes the old image.

A route clones exactly one immutable execution image (resident image or
partitioned bundle lease) and releases the publication read lock before search.
Already-acquired work may finish on that immutable image. New work admitted
after a confirmed mutation observes only the replacement or an unavailable
state. A manual rebuild failure keeps an already-current image; when no image
is current, it records the newest failure.

Hydration reads current confirmed records. It does not claim that those records belong to the historical image used by an older response. Path hydration uses the response's own complete profile and evidence. No graph revision or image version is introduced.

## State transitions

```text
open + valid referenced bundle         -> Available(reused image)
open + absent/invalid bundle, rebuild  -> Available(new image)
open + require-valid refusal/failure   -> Unavailable
Available + successful mutation/build -> Available(new image)
Available + committed/build failure   -> Unavailable
Unavailable + successful repair/build -> Available
Unavailable + failed rebuild          -> Unavailable(new reason)
Available + failed manual rebuild     -> Available(current image)
```

## Lock order

```text
request-ID registration / admission permit (routing only)
  -> published-state lock
  -> Catalog public operation and its internal locks
```

The request registry and admission mutex are never acquired while the published-state write lock is held. Route code registers the ID, acquires an RAII admission slot, briefly reads and clones the image, completes its byte reservation, and searches without a publication lock. Confirmed mutations and profiled hydration acquire publication state before entering the catalog. Within `Catalog`, the existing write mutex precedes exact-name write locks. Cancellation takes only the request registry. Health briefly reads publication state and then admission counters; no reverse path exists.

## Consequences

Publication is deliberately a full deterministic rebuild. Topology memory and per-route working memory are separate limits. Process-local clocks, request IDs, and counters are diagnostics, not durable graph identity. Decision 0007 supersedes the original restart detail: restart now reuses a fully validated current bundle and rebuilds from RocksDB when that rebuildable index is absent or invalid.
