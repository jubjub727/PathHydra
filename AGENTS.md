# Working on PathHydra

## Purpose

PathHydra is a BAML application backed by a Rust graph library/API. The Rust layer stores weighted directed graph data, returns exact context-adjusted routing results, and hydrates caller-specified records. Final graph composition is outside the Rust engine contract.

## Terms

- **Node:** a stored thing or value.
- **Relation:** a typed, directed connection between nodes.
- **Logical distance:** accumulated context-adjusted edge weight used for selection.
- **Routing result:** exact destination distances and optional path identities.
- **Subgraph:** a caller-owned, version-pinned set of node and relation handles assembled through the Rust API.
- **Hydration:** turning caller-specified node and relation handles into complete records.
- **Provisional candidate:** proposed graph material that is stored but excluded from confirmed lookup, routing, and hydration until externally validated and promoted.

Use these terms consistently in code and documentation.

## Priorities

1. Correct and inspectable results.
2. A small, clear data model.
3. Deterministic CPU behaviour as a reference.
4. GPU acceleration without changing semantics.
5. Performance claims backed by benchmarks.

## Working rules

- Do not hide graph semantics behind vague names such as `item`, `link`, or `data` when a precise term exists.
- Keep storage, routing, hydration, and caller-owned composition separate.
- Subgraph operations must not mutate confirmed or provisional database state.
- Reject attempts to combine subgraphs or handles from different graph versions.
- Treat relation direction and category as part of correctness.
- Treat node and relation names as exact, case-sensitive strings. Do not normalize, fold, correct, alias, or merge them.
- Use hashes to accelerate exact-name lookup, never as unverified identity. Hash collisions must compare the complete name.
- Never expose provisional candidates as confirmed graph data or include them in routing snapshots.
- Removing a node must atomically remove every incoming and outgoing relation plus its lookup records.
- Removing a relation must remove every durable and routing representation of that relation.
- Preserve the graph version, context profile, destinations, weights, and tie policy used for every routing result.
- Make search limits explicit: depth, fan-out, cycles, and budgets.
- Add a CPU reference implementation before or alongside GPU-specific work.
- Prefer small fixtures that make expected paths obvious.
- Document new formats and public interfaces when they are introduced.
- Do not add dependencies or abstractions without a current use for them.

## Definition of done

A change is done when its behaviour is tested, edge cases are covered, public behaviour is documented, and CPU/GPU routing results agree where both implementations exist.
