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

function Get-WebViewLoaderArchitecture {
    $hostLine = (& rustc -Vv | Select-String -Pattern "^host: ").Line
    $triple = if ($hostLine) { $hostLine.Replace("host: ", "").Trim() } else { "" }
    switch -Regex ($triple) {
        "^aarch64" { return "arm64" }
        "^i686" { return "x86" }
        "^x86_64" { return "x64" }
        default { return "x64" }
    }
}

$loaderArchitecture = Get-WebViewLoaderArchitecture
$builtLoader = Get-ChildItem -Path (Join-Path $releaseDirectory "build") `
    -Recurse -Filter "WebView2Loader.dll" -ErrorAction SilentlyContinue |
    Where-Object { $_.Directory.Name -eq $loaderArchitecture } | Select-Object -First 1
if (-not $builtLoader) {
    throw "WebView2Loader.dll for '$loaderArchitecture' is missing from target\release\build. Build with 'cargo build --release --locked -j 1 --features compat-webview2' first; the compatibility backend needs it beside sylphra.exe."
}
$webviewLoader = Join-Path $releaseDirectory "WebView2Loader.dll"
Copy-Item -LiteralPath $builtLoader.FullName -Destination $webviewLoader -Force
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
Copy-Item -LiteralPath $webviewLoader -Destination (Join-Path $destination "WebView2Loader.dll") -Force
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
