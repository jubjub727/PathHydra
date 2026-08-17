[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$ledgerPath = Join-Path $repositoryRoot "docs/system-conformance.md"
if (-not (Test-Path -LiteralPath $ledgerPath -PathType Leaf)) {
    throw "missing conformance ledger: $ledgerPath"
}

$ledger = [IO.File]::ReadAllText($ledgerPath)
$forbidden = [regex]::Matches(
    $ledger,
    '(?im)\b(TBD|TODO|placeholder|partial|not implemented)\b|deliberately left open|remains open|not yet'
)
if ($forbidden.Count -ne 0) {
    throw "conformance ledger contains a stale marker: $($forbidden[0].Value)"
}

$allowedStatuses = @("implemented", "selected policy", "explicitly outside system scope")
$allRows = [regex]::Matches($ledger, '(?m)^\| (SS-[A-Z0-9-]+) \|')
$rowMatches = [regex]::Matches($ledger, '(?m)^\| (SS-[A-Z0-9-]+) \|.*\| (implemented|selected policy|explicitly outside system scope) \|$')
if ($rowMatches.Count -eq 0) {
    throw "conformance ledger contains no machine-checkable SS rows"
}
if ($rowMatches.Count -ne $allRows.Count) {
    throw "one or more SS rows lack the complete schema or an allowed terminal status"
}
if ($rowMatches.Count -lt 141) {
    throw "the current system-shape inventory contains fewer than its 141 reviewed requirements"
}
$seen = [Collections.Generic.HashSet[string]]::new()
foreach ($row in $rowMatches) {
    $id = $row.Groups[1].Value
    if (-not $seen.Add($id)) {
        throw "duplicate conformance row ID: $id"
    }
    if ($allowedStatuses -notcontains $row.Groups[2].Value) {
        throw "invalid status for $id"
    }
}

$markdownDirectory = Split-Path -Parent $ledgerPath
foreach ($match in [regex]::Matches($ledger, '\[[^\]]+\]\((?<target>[^)]+)\)')) {
    $target = $match.Groups['target'].Value
    if ($target -match '^(https?://|#)') { continue }
    $withoutAnchor = ($target -split '#', 2)[0]
    $resolved = Join-Path $markdownDirectory $withoutAnchor
    if (-not (Test-Path -LiteralPath $resolved)) {
        throw "stale ledger link: $target"
    }
}

foreach ($match in [regex]::Matches($ledger, '`(?<path>[^`#]+\.(?:rs|md|ps1))#(?<symbol>[^`]+)`')) {
    $relativePath = $match.Groups['path'].Value.Replace('/', [IO.Path]::DirectorySeparatorChar)
    $resolved = Join-Path $repositoryRoot $relativePath
    if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) {
        throw "stale evidence path: $relativePath"
    }
    $symbol = $match.Groups['symbol'].Value
    if (-not (Select-String -LiteralPath $resolved -SimpleMatch $symbol -Quiet)) {
        throw "stale evidence symbol: $relativePath#$symbol"
    }
}

& (Join-Path $PSScriptRoot "generate-dependency-inventory.ps1") -Check
Write-Output "system conformance ledger is current ($($rowMatches.Count) rows)"
