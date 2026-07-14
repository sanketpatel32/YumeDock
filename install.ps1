# YumeDock one-line installer.
#
# Usage (in PowerShell):
#   irm https://raw.githubusercontent.com/sanketpatel32/YumeDock/main/install.ps1 | iex
#
# Downloads the latest YumeDock.exe from the GitHub "latest" rolling release,
# places it in %LOCALAPPDATA%\Programs\YumeDock, and creates a Start Menu
# shortcut. No installer, no admin prompt -- the .exe is portable.

$ErrorActionPreference = 'Stop'

$Repo = 'sanketpatel32/YumeDock'
$ReleaseTag = 'latest'
$ExeName = 'YumeDock.exe'

function Write-Step($m) { Write-Host ""; Write-Host $m -ForegroundColor Cyan }
function Write-Fail($m) {
    Write-Host ""
    Write-Host "YumeDock install failed: $m" -ForegroundColor Red
    Write-Host "Download manually: https://github.com/$Repo/releases" -ForegroundColor Yellow
    exit 1
}

Write-Host ""
Write-Host "  YumeDock installer" -ForegroundColor White
Write-Host "  Lightweight macOS-style dock and top bar for Windows 11" -ForegroundColor DarkGray
Write-Host ""

# --- Locate the YumeDock.exe asset in the latest release. ---------------------
Write-Step "Finding the latest YumeDock release..."
try {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/tags/$ReleaseTag" `
        -Headers @{ 'User-Agent' = 'YumeDock-installer' } -UseBasicParsing
} catch {
    Write-Fail "Could not reach the GitHub API. Check your connection.`nDetails: $_"
}

$asset = $release.assets | Where-Object { $_.name -eq $ExeName } | Select-Object -First 1
if (-not $asset) {
    Write-Fail "The 'latest' release has no '$ExeName'. Has a release been published yet? See https://github.com/$Repo/releases"
}

$url = $asset.browser_download_url
Write-Host "  build: $($release.name)" -ForegroundColor DarkGray
Write-Host "  $url" -ForegroundColor DarkGray

# --- Install location + stop any running copy. --------------------------------
$installDir = Join-Path $env:LOCALAPPDATA "Programs\YumeDock"
$exePath = Join-Path $installDir $ExeName

if (Test-Path $exePath) {
    # If YumeDock is running, stop it so the exe file isn't locked.
    $running = Get-Process -Name YumeDock -ErrorAction SilentlyContinue
    if ($running) {
        Write-Step "Stopping running YumeDock (this also restores the taskbar)..."
        $running | Stop-Process -Force
        Start-Sleep -Milliseconds 600
    }
}
New-Item -ItemType Directory -Force -Path $installDir | Out-Null

# --- Download. -----------------------------------------------------------------
Write-Step "Downloading ($([math]::Round($asset.size / 1MB, 1)) MB)..."
try {
    Invoke-WebRequest -Uri $url -OutFile $exePath -UseBasicParsing
} catch {
    Write-Fail "Download failed.`nDetails: $_"
}
if (-not (Test-Path $exePath)) { Write-Fail "The downloaded file was not written." }

# --- Start Menu shortcut. ------------------------------------------------------
Write-Step "Creating Start Menu shortcut..."
$startDir = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs"
$shortcutPath = Join-Path $startDir "YumeDock.lnk"
try {
    $shell = New-Object -ComObject WScript.Shell
    $sc = $shell.CreateShortcut($shortcutPath)
    $sc.TargetPath = $exePath
    $sc.WorkingDirectory = $installDir
    $sc.Description = "Lightweight macOS-style dock and top bar for Windows 11"
    $sc.Save()
} catch {
    Write-Host "  (could not create shortcut: $_)" -ForegroundColor DarkGray
}

Write-Host ""
Write-Host "  YumeDock installed." -ForegroundColor Green
Write-Host "  Launch it from the Start Menu (search 'YumeDock')." -ForegroundColor White
Write-Host ""
Write-Host "  IMPORTANT -- if YumeDock hides your taskbar, restore it with:" -ForegroundColor Yellow
Write-Host "    Ctrl + Alt + Shift + F12" -ForegroundColor Yellow
Write-Host "    or right-click the YumeDock top bar > 'Restore taskbar and quit'." -ForegroundColor Yellow
Write-Host ""
Write-Host "  First time? Run it in safe mode first:" -ForegroundColor DarkGray
Write-Host "    $exePath --safe-mode" -ForegroundColor DarkGray
Write-Host ""
