$ErrorActionPreference = 'Stop'

$toolchain = 'nightly-2024-02-17'
$expected = 'rustc 1.78.0-nightly (bccb9bbb4 2024-02-16)'
$actual = (& rustc "+$toolchain" --version).Trim()
if ($actual -ne $expected) {
    throw "Expected $expected; found $actual"
}

$components = & rustup component list --toolchain "${toolchain}-x86_64-pc-windows-msvc" --installed
if (-not ($components -match '^rust-src')) {
    throw "rust-src is not installed for $toolchain"
}

$targets = & rustc "+$toolchain" --print target-list
if ($targets -notcontains 'nvptx64-nvidia-cuda') {
    throw 'The pinned compiler does not expose nvptx64-nvidia-cuda'
}

Write-Output "Verified $actual with rust-src and nvptx64-nvidia-cuda support."
