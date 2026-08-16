# Decision 0004: Rust PTX and the CUDA Driver boundary

Status: accepted.

PathHydra's NVIDIA kernels are Rust `no_std` code compiled by the pinned
`nightly-2024-02-17` toolchain for `nvptx64-nvidia-cuda` and `sm_86`. The host
workspace remains on stable Rust 1.95.0. `build.rs` emits PTX into Cargo's
`OUT_DIR`; generated PTX is embedded in `pathhydra-cuda`, is not durable graph
data, and is not checked in as source truth.

The host dynamically loads the CUDA 13.3 Driver API through exactly cudarc
0.19.8. Deployment requires a compatible NVIDIA display/compute driver but no
CUDA toolkit, `nvcc`, NVRTC DLL, cuBLAS, or cuGraph. cudarc's `nvrtc` Cargo
feature is enabled only because its safe driver module uses the `Ptx` carrier
type behind that feature; PathHydra never calls runtime compilation.

Missing drivers, unsupported compute capability, PTX rejection, module-load
failure, and device errors are typed CUDA failures. CPU routing remains
available under permissive policies.
