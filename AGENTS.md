# Working on PathHydra

## Purpose

PathHydra represents knowledge as nodes joined by categorical relations. Its main job is to find, rank, and hydrate logical paths through that graph, with GPU acceleration where parallel traversal pays off.

## Terms

- **Node:** a stored thing or value.
- **Relation:** a typed, directed connection between nodes.
- **Path:** an ordered sequence of nodes and relations.
- **Hydration:** turning compact path results into complete, usable records.
- **Inference:** a conclusion produced from a path. Keep it distinct from stored facts.

Use these terms consistently in code and documentation.

## Priorities

1. Correct and inspectable results.
2. A small, clear data model.
3. Deterministic CPU behaviour as a reference.
4. GPU acceleration without changing semantics.
5. Performance claims backed by benchmarks.

## Working rules

- Do not hide graph semantics behind vague names such as `item`, `link`, or `data` when a precise term exists.
- Keep storage, traversal, ranking, and hydration separate.
- Treat relation direction and category as part of correctness.
- Preserve provenance for facts and inferred paths.
- Make traversal limits explicit: depth, fan-out, allowed relations, cycles, and ranking rules.
- Add a CPU reference implementation before or alongside GPU-specific work.
- Prefer small fixtures that make expected paths obvious.
- Document new formats and public interfaces when they are introduced.
- Do not add dependencies or abstractions without a current use for them.

## Definition of done

A change is done when its behaviour is tested, edge cases are covered, public behaviour is documented, and CPU/GPU results agree where both implementations exist.
