# CUDA build

The ordinary workspace is CPU-capable and needs no NVIDIA software. CUDA is an
explicit feature:

```powershell
powershell -File Scripts/verify-cuda-toolchain.ps1
cargo build -p pathhydra-engine --features cuda
```

The host uses stable Rust 1.95.0. `cuda-toolchain.toml` pins the separate
kernel compiler to `nightly-2024-02-17` with `rust-src`. Install that component
explicitly—Cargo builds never invoke `rustup` or download tools. The nested
kernel build uses `-Zbuild-std=core`, emits PTX directly for `sm_86` under the
outer Cargo `OUT_DIR`, and embeds those bytes. A separate target directory
prevents nested Cargo locking the host target directory.

cudarc 0.19.8 is pinned with dynamic CUDA 13.3 Driver API loading. Its license
is MIT OR Apache-2.0. No CUDA toolkit or `nvcc` is needed at runtime. The NVRTC
Cargo feature exposes cudarc's PTX carrier to its safe driver API, but
PathHydra never calls NVRTC and does not require the NVRTC library.

Build failures name the absent toolchain/component. Driver or PTX-JIT failures
occur at engine initialization and leave permissive configurations CPU-only.
Delete only Cargo's normal package artifacts for a clean rebuild; no script in
this repository deletes toolchains or driver state.

Real-device checks are:

```powershell
cargo test -p pathhydra-cuda --features cuda
cargo test -p pathhydra-engine --features cuda
cargo run -p pathhydra-bench --release --features cuda -- --suite baseline
```
