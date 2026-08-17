# Decision 0002: CPU routing arithmetic and tie policy

## Status

Accepted.

## Context

The immutable routing image is the reference input for exact CPU routing and a
CPU and GPU implementations. Request context must alter edge costs through a
compact, reproducible numeric contract. Equal-distance paths also need a
stable selection rule so returned edge identities are inspectable.

## Decision

`RelationMultiplier` stores a canonical IEEE-754 binary32 value. Finite values
greater than or equal to positive zero are accepted. Negative values, NaN, and
infinities are rejected. Negative zero is accepted at the API boundary and
canonicalized to positive zero. Subnormal values and the maximum finite
binary32 value are valid.

Zero is an enabled multiplier and can make an edge's effective weight zero. A
disabled relation kind is represented separately and is not traversable. A
request profile contains exactly one explicit enabled or disabled entry for
every confirmed relation kind. Duplicate, missing, and unknown relation IDs
are errors. Profile equality compares relation IDs, enabled/disabled states,
and canonical multiplier bits exactly.

For each enabled edge, the CPU reference performs two separate binary64
operations under round-to-nearest, ties-to-even:

```text
effective = f64(base_weight) * f64(relation_multiplier)
candidate = current_distance + effective
```

It does not use fused multiply-add or approximate comparison. Every product
and sum is checked for finiteness and a non-finite result is a typed arithmetic
failure. Unreachable and incomplete are explicit result states; public logical
distances never use NaN or infinity sentinels.

The stable predecessor tie policy is:

1. assign dense node IDs in ascending external `NodeId` order;
2. order each outgoing range by ascending `EdgeId`;
3. order the frontier by logical distance and then dense node ID;
4. before finalization, prefer the lowest lexicographic predecessor tuple
   `(predecessor dense node ID, edge ID)` for equal tentative distances; and
5. never reopen a finalized node for an equal-distance alternative.

This defines one deterministic minimum-distance predecessor tree. It does not
claim to return the globally lexicographically smallest path. Zero-weight
cycles terminate because each node is finalized at most once. The GPU
backend must reproduce this policy for requests it serves or leave those
requests on the CPU reference engine.

## Consequences

The numeric policy identifier is
`binary32-operands-separate-binary64-v1`. The tie-policy identifier is
`distance-dense-node-stable-predecessor-v1`. Routing images record both, and
responses identify the corresponding typed policies.
