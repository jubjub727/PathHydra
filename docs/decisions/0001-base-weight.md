# Decision 0001: Normalized base-weight representation

## Status

Accepted.

## Context

PathHydra relation weights are normalized logical distances. A value of `0`
is the closest possible distance and `1` is the furthest possible distance.
Stored values outside that range have no domain meaning. The durable format
must also remain directly usable by a deterministic CPU reference engine and a
later GPU backend.

## Decision

`BaseWeight` stores one IEEE-754 binary32 value in the inclusive range
`0.0..=1.0`.

- `0.0` and `1.0` are valid and are the exact minimum and maximum.
- Zero-weight edges and cycles are valid.
- Negative values, values above `1.0`, NaN, and positive or negative infinity
  are rejected before a candidate is written.
- Negative zero is accepted at the API boundary and stored as positive zero.
- The durable representation is the canonical four-byte IEEE-754 bit pattern
  in big-endian byte order.
- Exact equality of stored weights is equality of those canonical 32 bits.
  PathHydra does not apply approximate equality while comparing stored records.

Binary32 covers the complete domain range with substantially more fractional
resolution than the current normalized-distance contract requires. It is a
native scalar on CPUs and GPUs, avoids a conversion-only durable type, and can
be copied directly into the current routing snapshot. CPU/GPU routing agreement
will still be tested against one declared arithmetic policy when routing is
implemented.

## Resolved routing policies

This decision covers only the stored base weight. Decision 0002 subsequently
selected request multipliers, separate binary64 effective-weight
multiplication and path accumulation, rounding and overflow behavior, explicit
disabled relation kinds, unreachable-distance representation, and stable
predecessor tie handling. Decision 0013 closes the corresponding system-level
implementation choices. Those routing policies are not inferred from this
stored base-weight decision.
