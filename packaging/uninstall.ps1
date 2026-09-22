$ErrorActionPreference = "Stop"

$destinations = @(
    "$env:LOCALAPPDATA\Programs\Sylphra",
    "$env:LOCALAPPDATA\Programs\GhitaBrowser"
)
$desktop = [Environment]::GetFolderPath("Desktop")
$links = @(
    (Join-Path $desktop "Sylphra.lnk"),
    (Join-Path $desktop "GhitaBrowser.lnk")
)
$startMenus = @(
    "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\Sylphra.lnk",
    "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\GhitaBrowser.lnk"
)

foreach ($dest in $destinations) {
    if (Test-Path $dest) {
        Remove-Item -Recurse -Force $dest
        Write-Host "Removed: $dest"
    }
}

foreach ($lnk in $links) {
    if (Test-Path $lnk) {
        Remove-Item -Force $lnk
        Write-Host "Removed: $lnk"
    }
}

foreach ($sm in $startMenus) {
    if (Test-Path $sm) {
        Remove-Item -Force $sm
        Write-Host "Removed: $sm"
    }
}

Write-Host "Sylphra uninstalled successfully."
