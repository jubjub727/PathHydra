# PathHydra

PathHydra stores knowledge as things and the categorical relations between them, then finds logical paths through that graph.

Instead of returning a loose set of related records, it hydrates a path into an explicit chain:

```text
thing --relation--> thing --relation--> thing
```

The long-term goal is to explore many candidate paths in parallel on the GPU, rank the useful ones, and return reasoning that can be inspected rather than guessed at.

## Core model

- **Node:** a thing, concept, event, claim, or value.
- **Relation:** a typed connection between two nodes.
- **Path:** an ordered chain of nodes and relations.
- **Hydration:** resolving a discovered path into the full information needed to understand or use it.

## Direction

PathHydra is intended to provide:

- typed graph storage;
- constrained, GPU-accelerated pathfinding;
- path ranking and pruning;
- traceable inference results;
- a clean boundary between stored facts and inferred conclusions.

The project is in its early design stage. Interfaces and storage formats are not yet stable.
