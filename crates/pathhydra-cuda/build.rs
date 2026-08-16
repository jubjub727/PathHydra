use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const TOOLCHAIN: &str = "nightly-2024-02-17";
const TARGET: &str = "nvptx64-nvidia-cuda";
const TARGET_CPU: &str = "sm_86";

fn main() {
    println!("cargo:rerun-if-env-changed=PATHHYDRA_CUDA_SKIP_PTX_BUILD");
    for source in [
        "kernel/Cargo.toml",
        "kernel/lib.rs",
        "kernel/arithmetic.rs",
        "kernel/atomic.rs",
        "kernel/benchmark.rs",
        "kernel/frontier.rs",
        "kernel/delta.rs",
        "kernel/partition_frontier.rs",
        "kernel/partition_delta.rs",
        "../../cuda-toolchain.toml",
    ] {
        println!("cargo:rerun-if-changed={source}");
    }
    if env::var_os("CARGO_FEATURE_CUDA").is_none() {
        return;
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo supplies OUT_DIR"));
    if env::var_os("PATHHYDRA_CUDA_SKIP_PTX_BUILD").is_some() {
        let supplied = env::var_os("PATHHYDRA_CUDA_PTX")
            .map(PathBuf::from)
            .expect("PATHHYDRA_CUDA_PTX must name validated PTX when the build is skipped");
        fs::copy(supplied, out_dir.join("pathhydra.ptx"))
            .expect("failed to copy explicitly supplied PathHydra PTX");
        return;
    }

    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo supplies CARGO_MANIFEST_DIR"),
    );
    let kernel_dir = manifest_dir.join("kernel");
    let target_dir = out_dir.join("kernel-target");
    let status = Command::new("cargo")
        .current_dir(&kernel_dir)
        .env("CARGO_TARGET_DIR", &target_dir)
        .env_remove("RUSTC")
        .env_remove("RUSTDOC")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("RUSTUP_TOOLCHAIN")
        .args([
            format!("+{TOOLCHAIN}"),
            "rustc".to_owned(),
            "--release".to_owned(),
            "--target".to_owned(),
            TARGET.to_owned(),
            "-Zbuild-std=core".to_owned(),
            "--".to_owned(),
            "--emit=asm".to_owned(),
            "-C".to_owned(),
            format!("target-cpu={TARGET_CPU}"),
            "-C".to_owned(),
            "panic=abort".to_owned(),
        ])
        .status()
        .unwrap_or_else(|error| {
            panic!(
                "failed to run the pinned CUDA kernel toolchain {TOOLCHAIN}: {error}; run Scripts/verify-cuda-toolchain.ps1"
            )
        });
    assert!(
        status.success(),
        "Rust-to-PTX build failed; verify {TOOLCHAIN}, rust-src, and target support with Scripts/verify-cuda-toolchain.ps1"
    );

    let ptx = find_artifact(&target_dir).unwrap_or_else(|| {
        panic!(
            "the CUDA kernel build succeeded but emitted no PTX assembly under {}",
            target_dir.display()
        )
    });
    fs::copy(&ptx, out_dir.join("pathhydra.ptx"))
        .unwrap_or_else(|error| panic!("failed to stage PTX from {}: {error}", ptx.display()));
}

fn find_artifact(root: &Path) -> Option<PathBuf> {
    let mut directories = vec![root.to_owned()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.is_dir() {
                directories.push(path);
            } else if path.extension() == Some(OsStr::new("s"))
                && path
                    .file_stem()
                    .is_some_and(|stem| stem.to_string_lossy().starts_with("pathhydra_cuda_kernel"))
            {
                return Some(path);
            }
        }
    }
    None
}
