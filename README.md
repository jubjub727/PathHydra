# PathHydra

PathHydra stores knowledge as things and the categorical relations between them, then finds the closest logical region of that graph for the current context.

Inference does not return a chain. Context-weighted pathfinding is used to select the relevant part of the stored graph. Hydration returns that selection as a graph, preserving its branches and shared connections:

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
- **Result graph:** the selected network of nodes and relations, with no linear-path restriction.
- **Hydration:** resolving the selected graph into its full nodes and named relations.

## Direction

PathHydra is intended to provide:

- typed graph storage;
- constrained, GPU-accelerated pathfinding;
- exact context-weighted graph selection;
- traceable inference results;
- a clean boundary between stored facts and inferred conclusions.

The project is in its early design stage. Interfaces and storage formats are not yet stable.
