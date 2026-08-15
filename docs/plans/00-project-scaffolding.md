# Plan 00: Rust Project Scaffolding

## Outcome

Create the smallest useful Rust workspace for PathHydra's core layer. At completion, the repository builds, formats, lints, and tests from one command sequence. It contains only the crates needed by the next plan.

This plan sets up structure and guardrails. It does not implement graph behaviour.

## Scope

Create two crates:

- `pathhydra-core`: dependency-light domain types and invariants;
- `pathhydra-store`: RocksDB-backed persistence, depending on `pathhydra-core`.

The split is justified now: domain tests should not compile the native RocksDB dependency, while storage code needs a clear boundary around that dependency.

Do not create routing, GPU, API-server, BAML, or subgraph crates yet. Add them when implementation reaches those boundaries.

## Repository shape

```text
PathHydra/
  .github/
    workflows/
      rust.yml
  crates/
    pathhydra-core/
      Cargo.toml
      src/
        lib.rs
    pathhydra-store/
      Cargo.toml
      src/
        lib.rs
  docs/
    plans/
  Cargo.lock
  Cargo.toml
  rust-toolchain.toml
  AGENTS.md
  PATHHYDRA_SYSTEM_SHAPE.md
  README.md
```

## 1. Record the local toolchain

Verify the free local tools before creating files:

```powershell
git status --short --branch
rustup show
rustc --version
cargo --version
rustfmt --version
cargo clippy --version
cmake --version
clang --version
```

Install the current stable Rust toolchain through `rustup` if it is absent. Include `rustfmt` and `clippy`. Record the exact working Rust release in `rust-toolchain.toml` rather than leaving builds dependent on whichever release happens to be current later.

RocksDB's native build requirements are not exercised in this plan, but `cmake`, a working linker, and Clang/LLVM should be identified now so Plan 01 does not discover them halfway through implementation.

Do not install paid tooling or require a hosted service.

## 2. Create the workspace

Create a Cargo workspace at the repository root with resolver version 3 and Rust edition 2024. Commit `Cargo.lock` because this repository is an application system, not a published standalone library.

The root `Cargo.toml` should contain:

```toml
[workspace]
resolver = "3"
members = [
    "crates/pathhydra-core",
    "crates/pathhydra-store",
]

[workspace.package]
edition = "2024"
publish = false

[workspace.lints.rust]
unsafe_code = "forbid"
```

Do not add a Cargo licence field or repository licence file until the owner chooses one.

Create both crates as libraries with `--vcs none`. Each crate inherits workspace package fields and workspace lints.

`pathhydra-store` should declare a path dependency on `pathhydra-core`. Add no third-party dependencies yet.

## 3. Establish crate boundaries

Use crate-level documentation to state the boundary without adding placeholder abstractions:

- `pathhydra-core` owns IDs, exact names, records, lifecycle types, and errors that do not depend on RocksDB.
- `pathhydra-store` owns durable encodings, column families, atomic updates, cache rebuilding, and RocksDB-specific errors.

Both `lib.rs` files should compile with one minimal smoke test. Do not add empty modules for future routing or GPU work.

## 4. Add repository checks

The authoritative local checks are:

```powershell
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Add `.github/workflows/rust.yml` for the same checks on the public repository. Use the pinned toolchain from `rust-toolchain.toml`. The workflow must not call paid services, upload project data elsewhere, or become necessary for local development.

Keep the first workflow to one operating system. Add a Windows/Linux matrix only when the RocksDB crate is introduced and both native builds have been made reliable.

## 5. Align repository files

Extend `.gitignore` only for files the scaffold creates:

```text
/target/
```

Do not ignore `Cargo.lock`.

Add a short README development section containing the four local verification commands. Link both plan documents without describing unbuilt features as implemented.

Do not create BAML source, prompts, generated clients, or application directories in this plan.

## 6. Verify the scaffold

The scaffold is complete only when:

- `cargo metadata --no-deps` lists exactly the two intended crates;
- all four local checks pass from the repository root;
- neither crate contains unused placeholder modules;
- `pathhydra-core` has no third-party dependencies;
- `pathhydra-store` depends only on `pathhydra-core`;
- the CI workflow runs the same checks;
- the working tree contains no build output or environment files;
- the README describes the code as scaffolding, not as a working graph engine.

## 7. Commit boundary

Commit the scaffold as one reviewable change. Do not combine Plan 01 implementation with this commit.

Suggested commit message:

```text
Scaffold Rust core workspace
```

## Evidence behind the setup

- Cargo's official [workspaces documentation](https://doc.rust-lang.org/cargo/reference/workspaces.html) defines shared dependency resolution, package settings, and workspace commands.
- Rust's official [toolchain override documentation](https://rust-lang.github.io/rustup/overrides.html) describes repository-pinned toolchains through `rust-toolchain.toml`.
- The maintained [rust-rocksdb repository](https://github.com/rust-rocksdb/rust-rocksdb) confirms that RocksDB enters Rust through a native binding, which is why the storage dependency is isolated from the domain crate.
