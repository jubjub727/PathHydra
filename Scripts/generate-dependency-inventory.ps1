[CmdletBinding()]
param(
    [string]$OutputPath = "docs/dependency-inventory.tsv",
    [switch]$Check
)

$ErrorActionPreference = "Stop"
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$lockPath = Join-Path $repositoryRoot "Cargo.lock"
$resolvedOutput = if ([System.IO.Path]::IsPathRooted($OutputPath)) {
    $OutputPath
} else {
    Join-Path $repositoryRoot $OutputPath
}

$direct = @{}
foreach ($manifest in Get-ChildItem -LiteralPath (Join-Path $repositoryRoot "crates") -Filter Cargo.toml -Recurse) {
    $section = ""
    foreach ($line in Get-Content -LiteralPath $manifest.FullName) {
        if ($line -match '^\[(?<section>[^]]+)\]$') {
            $section = $Matches.section
            continue
        }
        if ($section -notmatch '(^|\.)((dev-|build-)?dependencies)$') { continue }
        if ($line -notmatch '^(?<name>[A-Za-z0-9_-]+)\s*=') { continue }
        $name = $Matches.name
        $kind = if ($section -match 'dev-dependencies$') {
            "dev"
        } elseif ($section -match 'build-dependencies$') {
            "build"
        } else {
            "runtime"
        }
        if (-not $direct.ContainsKey($name)) {
            $direct[$name] = [Collections.Generic.List[object]]::new()
        }
        $direct[$name].Add([pscustomobject]@{
            kind = $kind
            optional = $line -match 'optional\s*=\s*true'
            uses_default_features = $line -notmatch 'default-features\s*=\s*false'
        })
    }
}

$lockText = [IO.File]::ReadAllText($lockPath)
$packages = [Collections.Generic.List[object]]::new()
foreach ($match in [regex]::Matches($lockText, '(?ms)^\[\[package\]\]\s*(?<body>.*?)(?=^\[\[package\]\]|\z)')) {
    $body = $match.Groups['body'].Value
    if ($body -notmatch '(?m)^name\s*=\s*"(?<name>[^"]+)"\s*$') { continue }
    $name = $Matches.name
    if ($body -notmatch '(?m)^version\s*=\s*"(?<version>[^"]+)"\s*$') { continue }
    $version = $Matches.version
    $source = if ($body -match '(?m)^source\s*=\s*"(?<source>[^"]+)"\s*$') {
        $Matches.source
    } else {
        $null
    }
    if ($null -eq $source) { continue }
    $packages.Add([pscustomobject]@{ name = $name; version = $version; source = $source })
}

$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE ".cargo" }
$registryRoots = @(Get-ChildItem -LiteralPath (Join-Path $cargoHome "registry/src") -Directory -ErrorAction SilentlyContinue)
function Get-PackageLicense([string]$Name, [string]$Version) {
    foreach ($root in $registryRoots) {
        $manifest = Join-Path $root.FullName "$Name-$Version/Cargo.toml"
        if (-not (Test-Path -LiteralPath $manifest -PathType Leaf)) { continue }
        $text = [IO.File]::ReadAllText($manifest)
        if ($text -match '(?m)^license\s*=\s*"(?<license>[^"]+)"\s*$') {
            return $Matches.license
        }
        if ($text -match '(?m)^license-file\s*=\s*"(?<file>[^"]+)"\s*$') {
            return "license-file:$($Matches.file)"
        }
    }
    return "UNDECLARED"
}

function Get-Role([string]$Name, [bool]$IsDirect, [string]$Kinds) {
    if (-not $IsDirect) { return "transitive support" }
    switch ($Name) {
        "base64" { return "canonical payload encoding" }
        "blake3" { return "routing-bundle integrity" }
        "crossbeam-channel" { return "bounded partition I/O workers" }
        "cudarc" { return "optional CUDA Driver API" }
        "postcard" { return "development encoding comparison" }
        "rocksdb" { return "durable graph store" }
        "serde" { return "owned DTO serialization" }
        "serde_json" { return "canonical JSON boundary" }
        "tempfile" {
            if ($Kinds -eq "runtime") { return "benchmark scratch directories" }
            return "development and fault-test scratch directories"
        }
        default { return "direct $Kinds dependency" }
    }
}

function Get-NativeRequirement([string]$Name) {
    switch ($Name) {
        "rocksdb" { return "builds bundled RocksDB through librocksdb-sys" }
        "librocksdb-sys" { return "C++ compiler plus LLVM/libclang for bindgen" }
        "bindgen" { return "LLVM/libclang at build time" }
        "clang-sys" { return "LLVM/libclang at build time" }
        "cudarc" { return "compatible NVIDIA display/compute driver when cuda is enabled" }
        "blake3" { return "portable Rust fallback; target-specific SIMD selected by build" }
        "zstd-sys" { return "bundled native compression build pulled by RocksDB" }
        "lz4-sys" { return "bundled native compression build pulled by RocksDB" }
        "bzip2-sys" { return "bundled native compression build pulled by RocksDB" }
        default { return "none beyond Rust target toolchain" }
    }
}

$rows = [Collections.Generic.List[string]]::new()
$rows.Add("package`tversion`tsource`tlicense`trole`tdependency_status`tdefault_features`tnative_or_runtime_requirement")
foreach ($package in ($packages | Sort-Object name, version)) {
    $name = [string]$package.name
    $isDirect = $direct.ContainsKey($name)
    $declarations = if ($isDirect) { @($direct[$name]) } else { @() }
    $kinds = if (-not $isDirect) {
        "transitive"
    } else {
        $declaredKinds = @($declarations | ForEach-Object { [string]$_.kind } | Sort-Object -Unique)
        $declaredKinds -join "+"
    }
    $dependencyStatus = if (-not $isDirect) {
        "transitive"
    } elseif (@($declarations | Where-Object { -not $_.optional }).Count -gt 0) {
        "direct-required-$kinds"
    } else {
        "direct-optional-$kinds"
    }
    $defaultFeatures = if (-not $isDirect) {
        "inherited"
    } elseif (@($declarations | Where-Object uses_default_features).Count -gt 0) {
        "enabled"
    } else {
        "disabled"
    }
    $source = if ([string]$package.source -like "registry+*") {
        "crates.io"
    } else {
        [string]$package.source
    }
    $license = Get-PackageLicense $name ([string]$package.version)
    $fields = @(
        $name,
        [string]$package.version,
        $source,
        $license,
        (Get-Role $name $isDirect $kinds),
        $dependencyStatus,
        $defaultFeatures,
        (Get-NativeRequirement $name)
    ) | ForEach-Object { ([string]$_).Replace("`t", " ").Replace("`r", " ").Replace("`n", " ") }
    $rows.Add($fields -join "`t")
}

$lockHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $lockPath).Hash.ToLowerInvariant()
$content = @(
    "# Generated by Scripts/generate-dependency-inventory.ps1 from Cargo.lock, workspace manifests, and cached package manifests."
    "# Cargo.lock sha256: $lockHash"
) + $rows
$rendered = ($content -join "`n") + "`n"

if ($Check) {
    if (-not (Test-Path -LiteralPath $resolvedOutput -PathType Leaf)) {
        throw "dependency inventory is missing: $resolvedOutput"
    }
    $existing = [IO.File]::ReadAllText($resolvedOutput).Replace("`r`n", "`n")
    if ($existing -ne $rendered) {
        throw "dependency inventory is stale; run Scripts/generate-dependency-inventory.ps1"
    }
    Write-Output "dependency inventory is current ($($rows.Count - 1) external packages)"
    return
}

$parent = Split-Path -Parent $resolvedOutput
if (-not (Test-Path -LiteralPath $parent -PathType Container)) {
    New-Item -ItemType Directory -Path $parent | Out-Null
}
[IO.File]::WriteAllText($resolvedOutput, $rendered, [Text.UTF8Encoding]::new($false))
Write-Output "wrote $($rows.Count - 1) external packages to $resolvedOutput"
