# Decision 0005: Audited unsafe CUDA boundary

Status: accepted.

All existing crates continue to inherit `unsafe_code = "forbid"`.
`pathhydra-cuda` instead denies unsafe code crate-wide and locally permits it
only in private `src/launch.rs`; the separately compiled `kernel/` package is
also an explicit device boundary. `unsafe_op_in_unsafe_fn` and Clippy's
undocumented-unsafe-block lint are denied.

Every launch block documents the complete kernel ABI, fixed-width count
checks, buffer lengths, alignment and initialization, exclusive writes,
context/stream association, synchronization, and lifetime obligations. Raw
driver handles, device pointers, launch builders, and device buffers are not
public API. Public methods borrow synchronously or own their asynchronous job
inputs. An ABI mismatch is a correctness defect and a device-side invariant
status poisons the request rather than producing a partial response.
