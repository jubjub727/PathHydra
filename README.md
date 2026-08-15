# PathHydra

PathHydra stores knowledge as things and the categorical relations between them, then finds the closest logical region of that graph for the current context.

Context-weighted pathfinding exposes exact distances and, when requested, the paths that produced them. Those results can support branching graph structures without the Rust engine imposing one composition strategy:

```mermaid
flowchart LR
    origin["thing"] -->|relation| a["thing"]
    origin -->|relation| b["thing"]
    a -->|relation| c["thing"]
    a -->|relation| d["thing"]
    b -->|relation| d
    b -->|relation| e["thing"]
    c -->|relation| f["thing"]
    d -->|relation| f
    d -->|relation| g["thing"]
```

The long-term goal is to perform that selection in parallel on the GPU and return the hydrated subgraph with every included connection available for inspection.

## Core model

- **Node:** a thing, concept, event, claim, or value.
- **Relation:** a typed connection between two nodes.
- **Logical distance:** the accumulated context-adjusted weight used during graph selection.
- **Routing result:** exact destination distances and optional path identities.
- **Subgraph:** a caller-constructed set of nodes and relations assembled through the Rust API.
- **Hydration:** resolving caller-specified node and relation IDs into their full records.

## Direction

PathHydra is intended to provide:

- typed graph storage;
- constrained, GPU-accelerated pathfinding;
- exact context-weighted routing;
- version-safe subgraph construction;
- traceable inference results;
- a clean boundary between stored facts and inferred conclusions.

The project is in its early design stage. Interfaces and storage formats are not yet stable.

## Build plans

- [Project scaffolding](docs/plans/00-project-scaffolding.md)
- [First core slice: exact identity catalog](docs/plans/01-exact-identity-catalog.md)
