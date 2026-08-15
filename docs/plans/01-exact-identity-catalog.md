# Plan 01: Exact Identity Catalog

## Outcome

Build the first working vertical slice of the Rust core: durable exact-name catalogs for nodes and relation kinds, with provisional insertion and atomic confirmation.

At completion, the engine can:

- store a proposed node or relation name as a provisional candidate;
- keep provisional names out of confirmed lookup;
- confirm an externally validated candidate atomically;
- resolve confirmed names through complete, case-sensitive string equality;
- return stable numeric IDs;
- rebuild its in-memory hash indexes from RocksDB after restart;
- reject duplicate confirmed exact names without merging data implicitly.

This is the foundation for edges, deletion, snapshots, routing, hydration, and subgraphs.

## Explicit non-goals

Do not implement in this slice:

- edges or edge weights;
- node payload schemas;
- confirmed node deletion;
- routing or graph selection;
- routing snapshots or dense GPU IDs;
- subgraph construction;
- BAML code or validation behaviour;
- fuzzy lookup, aliases, normalization, or synonym handling;
- a network service or wire protocol.

Confirmed deletion begins with the edge-storage slice because its contract requires complete incident-edge removal. Do not expose an incomplete deletion API here.

## 1. Fix the public contracts with tests

Write failing integration tests before storage implementation. The fixtures must prove that all of these are distinct names:

```text
"token"
"Token"
"TOKEN"
"token!"
"token "
"tokén"
"tokén"
```

Do not trim, case-fold, normalize Unicode, remove punctuation, stem, or apply locale rules.

Test the required lifecycle:

1. Insert a provisional node candidate.
2. Confirmed lookup returns no match.
3. Read the candidate back exactly.
4. Confirm it through the explicit confirmation call.
5. Confirmed lookup returns its numeric ID.
6. Reopen the database.
7. The same exact lookup returns the same ID.

Repeat the lifecycle for a relation-kind candidate.

## 2. Add dependency versions deliberately

Add the maintained `rocksdb` crate only to `pathhydra-store`. Pin the selected compatible release in `Cargo.lock`. Start without optional compression features; name catalogs are too small to justify a codec choice before measurement.

Add `tempfile` as a development dependency of `pathhydra-store` for isolated database tests.

Do not add serialization, error-derive, concurrent-map, async-runtime, or logging crates in this slice. The standard library is sufficient for the initial types, hash maps, locks, and error implementations.

Before implementation, prove the native dependency builds with:

```powershell
cargo check -p pathhydra-store
```

If the build fails, fix the documented compiler, linker, CMake, or LLVM prerequisite. Do not replace RocksDB or add a second database to work around local setup.

## 3. Implement dependency-free domain types

Create these modules in `pathhydra-core`:

```text
src/
  candidate.rs
  id.rs
  name.rs
  lib.rs
```

Define opaque newtypes with private fields:

- `CandidateId(u64)`;
- `NodeId(u64)`;
- `RelationId(u64)`;
- `NodeName(Box<str>)`;
- `RelationName(Box<str>)`.

Name constructors preserve the supplied Rust string exactly. They must not trim or normalize it. Expose `as_str`, owned conversion, `Display`, `Debug`, `Eq`, `Hash`, and ordering traits needed by storage and tests.

Define provisional candidate values:

```text
Candidate::Node { id, name }
Candidate::Relation { id, name }
```

Define confirmed records containing their stable ID and exact name. Do not add payload fields until their encoding and update semantics are specified.

Use `u64` for durable IDs in this slice. Dense snapshot field widths remain a later physical-format decision.

## 4. Define a small durable format

Keep RocksDB encoding inside `pathhydra-store`:

```text
src/
  catalog.rs
  codec.rs
  column_families.rs
  error.rs
  lib.rs
```

Use one RocksDB database with these column families:

- default: next-ID counters;
- `candidates`;
- `nodes`;
- `node_names`;
- `relation_kinds`;
- `relation_names`.

This is a small fixed set with distinct access patterns. RocksDB supports atomic `WriteBatch` operations across column families in one database.

Encoding rules:

- numeric keys and values use fixed-width big-endian bytes;
- strings use an explicit byte length followed by their exact UTF-8 bytes;
- decoders reject truncation, trailing garbage, unknown candidate kinds, and invalid lengths;
- no Rust memory layout or debug representation is persisted;
- all decode failures identify the key space and record ID involved.

Write round-trip and malformed-input tests for every codec before catalog methods use it.

## 5. Implement durable metadata and ID allocation

On first open, initialize:

- next candidate ID;
- next node ID;
- next relation ID.

Serialize catalog writes through one store-owned mutex in this slice. Allocate an ID and update its counter in the same RocksDB write batch that creates the corresponding record. Check overflow and return a typed error instead of wrapping.

Candidate insertion does not affect confirmed lookup. Successful confirmation does.

## 6. Build exact-name hash indexes

Maintain two complete confirmed-name maps in memory:

```text
HashMap<Box<str>, NodeId>
HashMap<Box<str>, RelationId>
```

Use the standard-library `HashMap`. Hashing finds a candidate bucket; full `Eq` comparison decides identity. Never use the raw hash output as an ID.

At database open:

1. Iterate the confirmed name column families.
2. Decode every exact name and ID.
3. Verify the corresponding confirmed record exists and contains the same name.
4. Reject duplicate or inconsistent durable mappings.
5. Publish the fully rebuilt maps only after the scan succeeds.

The maps contain confirmed names only. A provisional candidate is never inserted into them.

Use a standard-library `RwLock` around the maps. Do not add a concurrent-map dependency until profiling shows lock contention.

## 7. Implement the catalog API

Expose a small synchronous API from `pathhydra-store`:

```text
Catalog::open(path)
Catalog::insert_node_candidate(name)
Catalog::insert_relation_candidate(name)
Catalog::get_candidate(candidate_id)
Catalog::confirm_validated_candidate(candidate_id)
Catalog::lookup_node_exact(name)
Catalog::lookup_relation_exact(name)
Catalog::get_node(node_id)
Catalog::get_relation(relation_id)
```

`confirm_validated_candidate` does not perform factual validation. Its name marks the boundary: the caller is asserting that validation already happened elsewhere.

Confirmation executes under the write mutex:

1. Load and decode the provisional candidate.
2. Check the durable exact-name index, not only the in-memory cache.
3. If the exact name already exists, return a typed `NameAlreadyConfirmed` error containing the existing ID. Do not merge or update either record.
4. Allocate the stable confirmed ID.
5. In one `WriteBatch`, create the confirmed record, create the exact-name mapping, remove the provisional candidate, and update the ID counter.
6. Commit the batch.
7. Update the in-memory map only after the durable commit succeeds.

All public errors distinguish not found, already confirmed, corrupt record, counter overflow, lock poisoning, and RocksDB failure.

## 8. Verify restart and failure behaviour

Add integration tests for:

- every case/spelling/Unicode fixture resolving independently;
- provisional candidates remaining absent from confirmed maps;
- node and relation namespaces allowing the same exact text independently;
- duplicate confirmation returning the existing ID without mutation;
- failed confirmation leaving the candidate provisional;
- close/reopen preserving IDs and exact bytes;
- cache rebuild detecting a missing confirmed record;
- malformed values returning errors rather than panicking;
- concurrent readers observing only confirmed map entries;
- ID-counter overflow returning an error;
- two simultaneous confirmation attempts for the same exact name producing at most one confirmed record.

Tests must use temporary directories and close every RocksDB handle before cleanup.

## 9. Document the implemented boundary

Add rustdoc examples for exact lookup and candidate confirmation. Update the README implementation-status section to state only what now works.

Record the column-family names, key encoding, value encodings, and confirmation batch in a short storage-layout document. Do not describe later edge or routing formats as settled.

## 10. Completion checks

Run:

```powershell
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Plan 01 is complete only when:

- all lifecycle and exact-name tests pass;
- opening a corrupt catalog fails visibly;
- no provisional name appears in confirmed lookup;
- confirmation is one durable atomic batch;
- a restart reconstructs identical confirmed maps;
- no BAML, routing, GPU, edge, or subgraph implementation has been added;
- the public API exposes no normalization or alias operation;
- the working tree is clean after the implementation commit.

Suggested commit message:

```text
Implement exact identity catalog
```

## Evidence behind the first slice

- Rust's standard-library [HashMap](https://doc.rust-lang.org/stable/std/collections/struct.HashMap.html) provides hashed lookup while key equality remains authoritative.
- RocksDB [column families](https://github.com/facebook/rocksdb/wiki/Column-Families) provide logical key-space separation and atomic writes across column families.
- RocksDB [basic operations](https://github.com/facebook/rocksdb/wiki/Basic-Operations) document atomic `WriteBatch` updates and snapshot reads.
- The [rust-rocksdb repository](https://github.com/rust-rocksdb/rust-rocksdb) provides the maintained Rust binding used by the storage crate.
