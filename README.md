# PathHydra

PathHydra is designed to store knowledge as nodes and typed, directed relations,
then find the closest logical region of that graph for the current context.

Context-weighted pathfinding exposes exact distances and, when requested, the paths that produced them. Those results can support branching graph structures without the Rust engine imposing one composition strategy:

```mermaid
flowchart LR
    origin["node"] -->|relation| a["node"]
    origin -->|relation| b["node"]
    a -->|relation| c["node"]
    a -->|relation| d["node"]
    b -->|relation| d
    b -->|relation| e["node"]
    c -->|relation| f["node"]
    d -->|relation| f
    d -->|relation| g["node"]
```

The long-term goal is to perform that selection in parallel on the GPU and return the hydrated subgraph with every included connection available for inspection.

## Core model

- **Node:** a thing, concept, event, claim, or value.
- **Relation:** a typed, directed connection between two nodes.
- **Logical distance:** the accumulated context-adjusted weight used during graph selection.
- **Routing result:** exact destination distances and optional path identities.
- **Subgraph:** a caller-owned, version-pinned set of node and relation handles assembled through the Rust API.
- **Hydration:** turning caller-specified node and relation handles into complete records.

## Direction

PathHydra is intended to provide:

- typed graph storage;
- constrained, GPU-accelerated pathfinding;
- exact context-weighted routing;
- version-safe subgraph construction;
- traceable inference results;
- a clean boundary between stored facts and inferred conclusions.

The project is in its early design stage. Interfaces and storage formats are not yet stable.

## Implementation status

The repository currently contains Rust workspace scaffolding only. The crates
define ownership boundaries for domain and storage code; graph storage,
routing, hydration, and caller-owned subgraph composition are not implemented.

## Development

Run the authoritative local checks from the repository root:

```powershell
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Build plans

- [Project scaffolding](docs/plans/00-project-scaffolding.md)
- [First core slice: exact identity catalog](docs/plans/01-exact-identity-catalog.md)
