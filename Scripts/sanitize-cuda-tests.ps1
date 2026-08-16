$ErrorActionPreference = 'Stop'

$sanitizer = Get-Command compute-sanitizer -ErrorAction SilentlyContinue
if ($null -eq $sanitizer) {
    throw 'NVIDIA Compute Sanitizer is not installed. Install an approved CUDA toolkit explicitly; it is not a runtime dependency.'
}

& $sanitizer.Source --tool memcheck --error-exitcode 99 --target-processes all cargo test -p pathhydra-cuda --features cuda --test agreement -- --test-threads=1
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

# WDDM can overflow Racecheck's launch tracker when the agreement suite
# deliberately submits from concurrent host threads. Blocking launches keeps
# the tool's kernel instrumentation complete; ordinary CUDA tests retain the
# concurrent scheduling coverage.
& $sanitizer.Source --tool racecheck --force-blocking-launches --error-exitcode 99 --target-processes all cargo test -p pathhydra-cuda --features cuda --test agreement -- --test-threads=1
exit $LASTEXITCODE
