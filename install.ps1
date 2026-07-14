# YumeDock installer bootstrap.
#
# One-line install (in PowerShell):
#   irm https://raw.githubusercontent.com/sanketpatel32/YumeDock/main/install.ps1 | iex
#
# This downloads the latest YumeDock-Setup.exe from the GitHub "latest" rolling
# release and runs it silently. It is per-user (no admin / UAC prompt).

# ErrorActionPreference only affects the current scope; irm | iex runs in a
# child scope so we set it defensively here too.
$ErrorActionPreference = 'Stop'

$Repo = 'sanketpatel32/YumeDock'
$ReleaseTag = 'latest'
$SetupName = 'YumeDock-Setup.exe'

function Write-Step($msg) {
    Write-Host ""
    Write-Host $msg -ForegroundColor Cyan
}

function Write-Fail($msg) {
    Write-Host ""
    Write-Host "YumeDock install failed: $msg" -ForegroundColor Red
    Write-Host "You can download it manually from https://github.com/$Repo/releases" -ForegroundColor Yellow
    exit 1
}

Write-Host ""
Write-Host "  YumeDock installer" -ForegroundColor White
Write-Host "  Lightweight macOS-style dock and top bar for Windows 11" -ForegroundColor DarkGray
Write-Host ""

# --- Resolve the latest installer download URL via the GitHub API. ---
Write-Step "Finding the latest YumeDock release..."
try {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/tags/$ReleaseTag" `
        -Headers @{ 'User-Agent' = 'YumeDock-installer' } `
        -UseBasicParsing
} catch {
    Write-Fail "Could not reach the GitHub API. Check your internet connection.`nDetails: $_"
}

$asset = $release.assets | Where-Object { $_.name -eq $SetupName } | Select-Object -First 1
if (-not $asset) {
    Write-Fail "The 'latest' release has no '$SetupName' asset yet. The first build may still be running; try again in a minute."
}

$downloadUrl = $asset.browser_download_url
Write-Host "  Found build: $($release.name)" -ForegroundColor DarkGray
Write-Host "  $downloadUrl" -ForegroundColor DarkGray

# --- Download the installer. ---
$tempDir = Join-Path $env:TEMP "YumeDock-install-$(Get-Random)"
New-Item -ItemType Directory -Force -Path $tempDir | Out-Null
$installerPath = Join-Path $tempDir $SetupName

Write-Step "Downloading installer ($([math]::Round($asset.size / 1MB, 1)) MB)..."
try {
    Invoke-WebRequest -Uri $downloadUrl -OutFile $installerPath -UseBasicParsing
} catch {
    Write-Fail "Download failed.`nDetails: $_"
}

if (-not (Test-Path $installerPath)) {
    Write-Fail "The downloaded installer was not written to disk."
}

# --- Run the installer silently (Inno Setup /SILENT). ---
Write-Step "Installing YumeDock (per-user, no admin needed)..."
try {
    $process = Start-Process -FilePath $installerPath -ArgumentList '/SILENT', '/SP-', '/NORESTART' -Wait -PassThru
    $exitCode = $process.ExitCode
} catch {
    Write-Fail "Could not start the installer.`nDetails: $_"
}

# Clean up the temp download.
Remove-Item -Recurse -Force $tempDir -ErrorAction SilentlyContinue

# Inno Setup returns 0 on success; 5 typically means the process was already
# running and closed.
if ($exitCode -ne 0 -and $exitCode -ne 5) {
    Write-Fail "Installer exited with code $exitCode."
}

Write-Host ""
Write-Host "  YumeDock installed successfully." -ForegroundColor Green
Write-Host "  Launch it from the Start Menu (search 'YumeDock')." -ForegroundColor White
Write-Host ""
Write-Host "  IMPORTANT: If YumeDock hides your taskbar and you need it back:" -ForegroundColor Yellow
Write-Host "    Press Ctrl + Alt + Shift + F12  (emergency restore + quit)" -ForegroundColor Yellow
Write-Host "    or right-click the YumeDock top bar > 'Restore taskbar and quit'." -ForegroundColor Yellow
Write-Host ""
