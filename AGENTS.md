# Working on PathHydra

## Purpose

PathHydra is a BAML application backed by a Rust graph library/API. It stores nodes joined by weighted categorical relations, uses context-adjusted shortest-path distance to select a relevant subgraph, and hydrates that graph without flattening it into a chain.

## Terms

- **Node:** a stored thing or value.
- **Relation:** a typed, directed connection between nodes.
- **Logical distance:** accumulated context-adjusted edge weight used for selection.
- **Selected graph:** the branching set of nodes and relations returned by inference.
- **Hydration:** turning compact graph-selection results into complete, usable records.

Use these terms consistently in code and documentation.

## Priorities

1. Correct and inspectable results.
2. A small, clear data model.
3. Deterministic CPU behaviour as a reference.
4. GPU acceleration without changing semantics.
5. Performance claims backed by benchmarks.

## Working rules

- Do not hide graph semantics behind vague names such as `item`, `link`, or `data` when a precise term exists.
- Keep storage, graph selection, and hydration separate.
- Treat relation direction and category as part of correctness.
- Preserve the graph version, context profile, weights, and selection boundary used for every result.
- Make search and graph-selection limits explicit: depth, fan-out, cycles, boundaries, and budgets.
- Add a CPU reference implementation before or alongside GPU-specific work.
- Prefer small fixtures that make expected paths obvious.
- Document new formats and public interfaces when they are introduced.
- Do not add dependencies or abstractions without a current use for them.

## Definition of done

A change is done when its behaviour is tested, edge cases are covered, public behaviour is documented, and CPU/GPU graph selections agree where both implementations exist.
