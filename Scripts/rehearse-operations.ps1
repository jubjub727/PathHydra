param(
    [Parameter(Mandatory = $true)]
    [string]$RehearsalRoot,
    [switch]$Cuda
)

$ErrorActionPreference = "Stop"
$resolvedRoot = [System.IO.Path]::GetFullPath($RehearsalRoot)
$filesystemRoot = [System.IO.Path]::GetPathRoot($resolvedRoot)
if ($resolvedRoot -eq $filesystemRoot) {
    throw "The rehearsal root must not be a filesystem root."
}
if (Test-Path -LiteralPath $resolvedRoot) {
    if (-not (Test-Path -LiteralPath $resolvedRoot -PathType Container)) {
        throw "The rehearsal root must be a directory."
    }
    if (Get-ChildItem -LiteralPath $resolvedRoot -Force | Select-Object -First 1) {
        throw "The rehearsal root must be empty."
    }
} else {
    New-Item -ItemType Directory -Path $resolvedRoot | Out-Null
}

$repository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$manifest = Join-Path $repository "crates\pathhydra-admin\Cargo.toml"
$workloadRoot = Join-Path $resolvedRoot "synthetic-workload"
$database = Join-Path $workloadRoot "database"
$checkpoint = Join-Path $resolvedRoot "checkpoint"
$restore = Join-Path $resolvedRoot "restore"
$restoreRouting = Join-Path $resolvedRoot "restore-routing"

function Invoke-Admin {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]]$AdminArguments)
    $output = & cargo run --quiet --release --manifest-path $manifest -- @AdminArguments
    if ($LASTEXITCODE -ne 0) {
        throw "pathhydra-admin failed for the requested rehearsal step."
    }
    return ($output | ConvertFrom-Json)
}

function Get-DirectoryFingerprint {
    param([string]$Path)
    return @(
        Get-ChildItem -LiteralPath $Path -Force -Recurse |
            Sort-Object FullName |
            ForEach-Object {
                $relative = $_.FullName.Substring($Path.Length).TrimStart(
                    [char[]]@(
                        [System.IO.Path]::DirectorySeparatorChar,
                        [System.IO.Path]::AltDirectorySeparatorChar
                    )
                )
                [pscustomobject]@{
                    Relative = $relative
                    Length = if ($_.PSIsContainer) { 0 } else { $_.Length }
                }
            }
    ) | ConvertTo-Json -Compress
}

Write-Host "Creating and verifying deterministic provisional and confirmed state..."
$workload = Invoke-Admin workload --root $workloadRoot --scale 32 --samples 5
if (-not $workload.measurements -or -not $workload.catalog_checksum) {
    throw "The synthetic workload did not return correctness evidence."
}
$summary = Invoke-Admin summary --database $database
if ($summary.catalog.confirmed_nodes -le 0 -or $summary.catalog.candidates.total -le 0) {
    throw "The rehearsal database must contain confirmed and provisional material."
}

Write-Host "Creating a database-only checkpoint..."
$checkpointReport = Invoke-Admin checkpoint-create `
    --database $database `
    --destination-root $resolvedRoot `
    --destination $checkpoint `
    --available-bytes ([UInt64]::MaxValue).ToString() `
    --headroom-bytes 0
if ($checkpointReport.files -le 0 -or $checkpointReport.bytes -le 0) {
    throw "The checkpoint report is incomplete."
}

Write-Host "Proving checkpoint disk admission refuses without creating a destination..."
$refusedCheckpoint = Join-Path $resolvedRoot "refused-checkpoint"
$previousErrorActionPreference = $ErrorActionPreference
$ErrorActionPreference = "Continue"
& cargo run --quiet --release --manifest-path $manifest -- `
    checkpoint-create `
    --database $database `
    --destination-root $resolvedRoot `
    --destination $refusedCheckpoint `
    --available-bytes 0 `
    --headroom-bytes 0 2>$null | Out-Null
$refusedCheckpointExitCode = $LASTEXITCODE
$ErrorActionPreference = $previousErrorActionPreference
if ($refusedCheckpointExitCode -eq 0 -or (Test-Path -LiteralPath $refusedCheckpoint)) {
    throw "A checkpoint with refused disk admission created a destination."
}

Write-Host "Validating restore into a fresh destination..."
$restoreReport = Invoke-Admin restore-validate `
    --source-root $resolvedRoot `
    --source $checkpoint `
    --destination-root $resolvedRoot `
    --destination $restore `
    --routing-root $restoreRouting `
    --available-bytes ([UInt64]::MaxValue).ToString() `
    --headroom-bytes 0
if ($restoreReport.catalog.confirmed_nodes -ne $summary.catalog.confirmed_nodes -or
    $restoreReport.catalog.candidates.total -ne $summary.catalog.candidates.total) {
    throw "Restored provisional/confirmed aggregate counts differ from the source."
}

Write-Host "Proving a failed restore leaves its source and destination marker untouched..."
$sourceBefore = Get-DirectoryFingerprint -Path $checkpoint
$refusal = Join-Path $resolvedRoot "refused-restore"
New-Item -ItemType Directory -Path $refusal | Out-Null
$marker = Join-Path $refusal "operator-owned-marker"
[System.IO.File]::WriteAllText($marker, "preserve")
$previousErrorActionPreference = $ErrorActionPreference
$ErrorActionPreference = "Continue"
& cargo run --quiet --release --manifest-path $manifest -- `
    restore-validate `
    --source-root $resolvedRoot `
    --source $checkpoint `
    --destination-root $resolvedRoot `
    --destination $refusal `
    --routing-root (Join-Path $resolvedRoot "refused-restore-routing") `
    --available-bytes ([UInt64]::MaxValue).ToString() 2>$null | Out-Null
$refusedRestoreExitCode = $LASTEXITCODE
$ErrorActionPreference = $previousErrorActionPreference
if ($refusedRestoreExitCode -eq 0) {
    throw "A restore into a nonempty destination unexpectedly succeeded."
}
if ((Get-DirectoryFingerprint -Path $checkpoint) -ne $sourceBefore -or
    [System.IO.File]::ReadAllText($marker) -ne "preserve") {
    throw "A refused restore changed its source or destination marker."
}

Write-Host "Rehearsing abrupt restart and Plan 06 durable publication recovery..."
Push-Location $repository
try {
    & cargo test -p pathhydra-engine --test durable_partitioned abrupt_process_exit_after_pointer_commit_reopens_the_complete_bundle -- --exact
    if ($LASTEXITCODE -ne 0) { throw "Abrupt restart rehearsal failed." }

    Write-Host "Rehearsing shutdown cancellation during a real partitioned route and successful retry..."
    & cargo test -p pathhydra-engine shutdown_cancels_a_real_active_partitioned_route_and_retry_joins_io
    if ($LASTEXITCODE -ne 0) { throw "Active-work shutdown rehearsal failed." }

    Write-Host "Rehearsing restored routing rebuild, route, hydration, and handle release..."
    & cargo test -p pathhydra-engine --test operations offline_restore_rebuilds_routing_smokes_and_preserves_candidates -- --exact
    if ($LASTEXITCODE -ne 0) { throw "Restored-engine rehearsal failed." }

    if ($Cuda) {
        Write-Host "Rehearsing restored CUDA initialization and exact routing..."
        & cargo test -p pathhydra-engine --features cuda --test operations cuda_restore_reinitializes_and_routes_exactly -- --exact
        if ($LASTEXITCODE -ne 0) { throw "CUDA restore rehearsal failed." }

        Write-Host "Rehearsing shutdown with one active and one queued CUDA route..."
        & cargo test -p pathhydra-engine --features cuda --test cuda_engine shutdown_cancels_real_queued_and_active_cuda_routes_and_joins_worker -- --exact
        if ($LASTEXITCODE -ne 0) { throw "Queued/active CUDA shutdown rehearsal failed." }
    }
} finally {
    Pop-Location
}

Write-Host "Rehearsing operator cutover and rollback validation without changing service configuration..."
$restoredSummary = Invoke-Admin summary --database $restore
$rollbackSummary = Invoke-Admin summary --database $database
if ($restoredSummary.catalog.confirmed_nodes -ne $rollbackSummary.catalog.confirmed_nodes -or
    $restoredSummary.catalog.candidates.total -ne $rollbackSummary.catalog.candidates.total) {
    throw "Cutover/rollback catalog aggregates disagree."
}

Write-Host "Rehearsal complete for store and engine operations."
Write-Host "CUTOVER (operator action only): explicitly shut down the live engine, smoke-test $restore, then atomically change service configuration to that path."
Write-Host "ROLLBACK (operator action only): explicitly shut down, point configuration back to the retained old directory, and start exactly one owner."
