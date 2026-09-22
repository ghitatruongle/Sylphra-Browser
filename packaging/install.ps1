$ErrorActionPreference = "Stop"

$projectRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$metadata = cargo metadata --no-deps --locked --format-version 1 --manifest-path (Join-Path $projectRoot "Cargo.toml") | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed" }
$package = $metadata.packages | Where-Object { $_.name -eq "sylphra" } | Select-Object -First 1
if (-not $package) {
    throw "Unable to read the Sylphra package version."
}

$version = $package.version
$sourceIcon = Join-Path $projectRoot "assets\icon.ico"
$destination = Join-Path $env:LOCALAPPDATA "Programs\Sylphra"
$releaseDirectory = Join-Path $projectRoot "target\release"
$binaries = [ordered]@{
    "sylphra.exe" = "sylphra.exe"
    "sylphra-renderer-worker.exe" = "sylphra-renderer-worker.exe"
    "sylphra-browser-child.exe" = "sylphra-browser-child.exe"
}

foreach ($sourceName in $binaries.Keys) {
    $source = Join-Path $releaseDirectory $sourceName
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
        throw "Release build file not found: $source`nRun 'cargo build --release --locked -j 1' first."
    }
    $productVersion = (Get-Item -LiteralPath $source).VersionInfo.ProductVersion
    if (-not ([string]$productVersion).StartsWith($version, [StringComparison]::Ordinal)) {
        throw "Binary '$source' has ProductVersion '$productVersion', expected '$version'."
    }
}

New-Item -ItemType Directory -Force -Path $destination | Out-Null
foreach ($sourceName in $binaries.Keys) {
    Copy-Item -LiteralPath (Join-Path $releaseDirectory $sourceName) -Destination (Join-Path $destination $binaries[$sourceName]) -Force
}
Copy-Item -LiteralPath $sourceIcon -Destination (Join-Path $destination "icon.ico") -Force

$installedExe = Join-Path $destination "sylphra.exe"
$desktop = [Environment]::GetFolderPath("Desktop")
$shortcutPath = Join-Path $desktop "Sylphra.lnk"
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut($shortcutPath)
$shortcut.TargetPath = $installedExe
$shortcut.WorkingDirectory = $destination
$shortcut.Description = "Sylphra v$version"
$shortcut.IconLocation = "$(Join-Path $destination 'icon.ico'),0"
$shortcut.Save()

Write-Host "Installed Sylphra v$version to $installedExe"
Write-Host "Shortcut: $shortcutPath"
