param(
    [ValidateSet("fast", "release", "full")]
    [string]$Tier = "fast",
    [switch]$Package
)

$ErrorActionPreference = "Stop"
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$passed = $false
$commandMetrics = [Collections.Generic.List[object]]::new()

if ($Package -and $Tier -ne "release") {
    throw "-Package is only valid with -Tier release"
}

function Get-DirectoryBytes {
    param([Parameter(Mandatory = $true)][string]$Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) { return [uint64]0 }
    $sum = (Get-ChildItem -LiteralPath $Path -Recurse -File -ErrorAction SilentlyContinue |
        Measure-Object -Property Length -Sum).Sum
    if ($null -eq $sum) { return [uint64]0 }
    return [uint64]$sum
}

function Invoke-Checked {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][scriptblock]$Command
    )
    Write-Host "[test:$Tier] $Name"
    $timer = [Diagnostics.Stopwatch]::StartNew()
    try {
        $global:LASTEXITCODE = 0
        & $Command
        if ($LASTEXITCODE -ne 0) {
            throw "$Name failed with exit code $LASTEXITCODE"
        }
        $commandMetrics.Add([ordered]@{
            name = $Name
            passed = $true
            elapsed_seconds = [math]::Round($timer.Elapsed.TotalSeconds, 3)
        })
    } catch {
        $commandMetrics.Add([ordered]@{
            name = $Name
            passed = $false
            elapsed_seconds = [math]::Round($timer.Elapsed.TotalSeconds, 3)
        })
        throw
    } finally {
        $timer.Stop()
    }
}

Push-Location $projectRoot
try {
    $metadata = cargo metadata --format-version 1 --no-deps --locked | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed" }
    $targetDirectory = if ($env:CARGO_TARGET_DIR) {
        [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)
    } else {
        [IO.Path]::GetFullPath($metadata.target_directory)
    }
    $targetRoot = [IO.Path]::GetPathRoot($targetDirectory)
    $driveName = $targetRoot.TrimEnd('\').TrimEnd(':')
    $freeBytesBefore = [uint64](Get-PSDrive -Name $driveName).Free
    if ($freeBytesBefore -lt 6GB) {
        throw "Need at least 6 GB free before compile; target is $targetDirectory and only $([math]::Round($freeBytesBefore / 1GB, 2)) GB is free"
    }

    $env:CARGO_BUILD_JOBS = "1"
    $env:CARGO_INCREMENTAL = "0"
    # 32 MB: rustc worker threads overflowed the smaller 16 MB stack while
    # compiling the larger integration-test crates on low-RAM machines.
    $env:RUST_MIN_STACK = "33554432"

    if ($Tier -in @("fast", "release")) {
        Invoke-Checked "Formatting" { cargo fmt --all -- --check }
        Invoke-Checked "Library tests" { cargo test --lib --locked -j 1 }
    }

    if ($Tier -eq "release") {
        Invoke-Checked "Acceptance suite" { cargo test --test acceptance --locked -j 1 }
        Invoke-Checked "Engine suite" { cargo test --test engine --locked -j 1 }
        Invoke-Checked "Network suite" { cargo test --test net --locked -j 1 }
        Invoke-Checked "Media suite" { cargo test --test media --locked -j 1 }
        Invoke-Checked "Storage suite" { cargo test --test storage --locked -j 1 }
        Invoke-Checked "UI suite" { cargo test --test ui --locked -j 1 }
        Invoke-Checked "Platform suite" { cargo test --test platform --locked -j 1 }
        Invoke-Checked "Release build" { cargo build --release --locked -j 1 }
        Invoke-Checked "Release smoke" { & (Join-Path $projectRoot "tools\release-smoke.ps1") }
        if ($Package) {
            Invoke-Checked "Release packaging" { & (Join-Path $projectRoot "packaging\package.ps1") -SkipBuild }
        }
    }

    if ($Tier -eq "full") {
        Invoke-Checked "Formatting" { cargo fmt --all -- --check }
        Invoke-Checked "All-target tests" { cargo test --all-targets --locked -j 1 }
        Invoke-Checked "Clippy" { cargo clippy --all-targets --all-features --locked -j 1 -- -D warnings }
        Invoke-Checked "License metadata audit" { & (Join-Path $projectRoot "tools\audit-licenses.ps1") }
        Invoke-Checked "RustSec vulnerability audit" { & (Join-Path $projectRoot "tools\audit-rustsec.ps1") }
    }

    $passed = $true
    Write-Host "[test:$Tier] PASS"
} finally {
    $stopwatch.Stop()
    if ($targetDirectory) {
        try {
            $freeBytesAfter = [uint64](Get-PSDrive -Name $driveName).Free
            $metrics = [ordered]@{
                schema_version = 1
                tier = $Tier
                passed = $passed
                started_at_utc = $startedAt.ToString("o")
                elapsed_seconds = [math]::Round($stopwatch.Elapsed.TotalSeconds, 3)
                target_directory = $targetDirectory
                target_bytes = Get-DirectoryBytes -Path $targetDirectory
                debug_bytes = Get-DirectoryBytes -Path (Join-Path $targetDirectory "debug")
                free_bytes_before = $freeBytesBefore
                free_bytes_after = $freeBytesAfter
                commands = @($commandMetrics)
            }
            $metricsDirectory = Join-Path $projectRoot "dist\build-metrics"
            New-Item -ItemType Directory -Force -Path $metricsDirectory | Out-Null
            $stamp = $startedAt.ToString("yyyyMMddTHHmmssfffZ")
            $metricsPath = Join-Path $metricsDirectory "$stamp-$Tier.json"
            $metrics | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $metricsPath -Encoding utf8
            Write-Host "[test:$Tier] Metrics: $metricsPath"
        } catch {
            Write-Warning "Unable to write build metrics: $($_.Exception.Message)"
        }
    }
    Pop-Location
}
