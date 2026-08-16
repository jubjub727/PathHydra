$ErrorActionPreference = 'Stop'

$sanitizer = Get-Command compute-sanitizer -ErrorAction SilentlyContinue
if ($null -eq $sanitizer) {
    throw 'NVIDIA Compute Sanitizer is not installed. Install an approved CUDA toolkit explicitly; it is not a runtime dependency.'
}

& $sanitizer.Source --tool memcheck --error-exitcode 99 --target-processes all cargo test -p pathhydra-cuda --features cuda --test agreement
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

& $sanitizer.Source --tool racecheck --error-exitcode 99 --target-processes all cargo test -p pathhydra-cuda --features cuda --test agreement
exit $LASTEXITCODE
