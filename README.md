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
- **Subgraph:** a caller-owned set of node and relation handles assembled through the Rust API.
- **Hydration:** turning caller-specified node and relation handles into complete records.

## Direction

PathHydra is intended to provide:

- typed graph storage;
- constrained, GPU-accelerated pathfinding;
- exact context-weighted routing;
- structurally safe subgraph construction;
- traceable inference results;
- a clean boundary between stored facts and inferred conclusions.

The project is in its early design stage. Interfaces and storage formats are not yet stable.

## Implementation status

The Rust core now provides a durable graph store. It preserves
opaque node payloads; exact node and relation-kind names; provisional node,
relation-kind, and edge candidates; and confirmed typed, directed, normalized
weighted edges. Promotion and deletion are atomic, parallel and self-edges
have independent identities, node deletion cascades through both adjacency
directions. Startup validates every confirmed record and index relationship.

Routing, GPU acceleration, hydration, and caller-owned subgraph composition
are not implemented yet. The current catalog layout is documented in
[the storage-format reference](docs/storage-format.md).

## Development

Building `pathhydra-store` requires a C++ toolchain and LLVM/libclang because
the RocksDB binding compiles native code and generates bindings. On Windows,
ensure LLVM's `bin` directory is on `PATH` (or set `LIBCLANG_PATH`).
If the `librocksdb-sys` build helper exits with `STATUS_DLL_NOT_FOUND`, ensure
the selected LLVM installation's `libclang.dll` and its native runtime are
discoverable on `PATH`; this is a toolchain startup failure, not a graph-store
failure.

Run the authoritative local checks from the repository root:

```powershell
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
