# YumeDock local release builder.
#
# Builds the release exe (with embedded icon), zips it, and publishes a GitHub
# Release. No CI, no pipeline -- run this on your own PC whenever you want a new
# release.
#
# Usage:
#   .\build-release.ps1                # auto version from Cargo.toml + commit
#   .\build-release.ps1 -Tag v0.2.0    # explicit tag/version
#   .\build-release.ps1 -Latest        # publish as the rolling "latest" release
#                                     # (the one install.ps1 reads from)
#
# Requirements:
#   - Rust toolchain (cargo)
#   - Python 3 (for the app icon)
#   - GitHub CLI (gh), authenticated via `gh auth login`
#   - git, with the repo pushed to origin (sanketpatel32/YumeDock)
#
# What it produces:
#   - target\release\YumeDock.exe   (the portable binary)
#   - dist\YumeDock-<version>.zip   (exe + a short install note)
#   - A GitHub Release with both files attached.

[CmdletBinding()]
param(
    [string]$Tag = "",
    [switch]$Latest
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $repoRoot

function Step($msg) { Write-Host ""; Write-Host "-> $msg" -ForegroundColor Cyan }
function Fail($msg) { Write-Host ""; Write-Host "build-release failed: $msg" -ForegroundColor Red; exit 1 }

# --- 1. Version / tag resolution ------------------------------------------------
Step "Resolving version..."
$cargoVersion = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' |
    Select-Object -First 1).Matches.Groups[1].Value

if ($Tag) {
    $version = $Tag.TrimStart('v')
    $tagName = if ($Tag -match '^v') { $Tag } else { "v$Tag" }
} else {
    $sha = (git rev-parse --short HEAD).Trim()
    $version = "$cargoVersion+$sha"
    $tagName = "v$version"
}

if ($Latest) { $tagName = "latest"; $version = "latest" }
Write-Host "  version: $version   tag: $tagName"

# --- 2. Build the icon ----------------------------------------------------------
Step "Generating app icon..."
python assets/make_icon.py
if ($LASTEXITCODE -ne 0) { Fail "icon generation failed" }

# --- 3. Build the release binary ------------------------------------------------
Step "Building release binary (cargo build --release)..."
cargo build --release
if ($LASTEXITCODE -ne 0) { Fail "cargo build failed" }

$exe = Join-Path $repoRoot "target\release\YumeDock.exe"
if (-not (Test-Path $exe)) { Fail "YumeDock.exe not found at $exe" }
$sizeMb = [math]::Round((Get-Item $exe).Length / 1MB, 2)
Write-Host "  built: $exe ($sizeMb MB)"

# --- 4. Zip the exe + a short note ---------------------------------------------
Step "Packaging zip..."
$dist = Join-Path $repoRoot "dist"
if (Test-Path $dist) { Remove-Item -Recurse -Force $dist }
New-Item -ItemType Directory -Force -Path $dist | Out-Null

$stage = Join-Path $dist "stage\YumeDock"
New-Item -ItemType Directory -Force -Path $stage | Out-Null
Copy-Item $exe $stage

$note = @"
YumeDock $version
=================

Run:
  YumeDock.exe              -- full mode (replaces the Windows taskbar)
  YumeDock.exe --safe-mode  -- show dock+bar but keep the Windows taskbar

Emergency restore (if your taskbar is hidden):
  Ctrl + Alt + Shift + F12

Config: %LOCALAPPDATA%\YumeDock\config.json
"@
Set-Content -Path (Join-Path $stage "README.txt") -Value $note -Encoding UTF8

$zipName = "YumeDock-$version.zip"
$zipPath = Join-Path $dist $zipName
Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $zipPath -Force
Remove-Item -Recurse -Force (Join-Path $dist "stage")
Write-Host "  zip: $zipPath"

# --- 5. Publish the GitHub Release ---------------------------------------------
Step "Publishing GitHub Release (tag $tagName)..."
# Verify gh is available and authenticated.
$null = gh --version 2>&1
if ($LASTEXITCODE -ne 0) { Fail "GitHub CLI (gh) not found. Install it and run 'gh auth login'." }

$remote = (git remote get-url origin)
Write-Host "  remote: $remote"

# Delete an existing release at this tag so re-runs are clean. The `view` call
# writes "release not found" to stderr when absent; under
# $ErrorActionPreference=Stop (WinPS 5.1) that becomes a terminating error, so
# guard it with a try/catch and a local relaxed error preference.
$alreadyExists = $false
try {
    $prevPref = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    $null = gh release view $tagName --json url 2>$null
    if ($LASTEXITCODE -eq 0) { $alreadyExists = $true }
    $ErrorActionPreference = $prevPref
} catch {
    $ErrorActionPreference = $prevPref
}
if ($alreadyExists) {
    Write-Host "  deleting existing release at $tagName..."
    gh release delete $tagName --cleanup-tag --yes
    if ($LASTEXITCODE -ne 0) { Fail "could not delete existing release" }
}

$isPrerelease = $Latest
$notes = "YumeDock $version.`n`n- **YumeDock.exe** - portable binary, run directly.`n- **$zipName** - same binary + README.txt.`n`nInstall with one line:`n``````powershell`nirm https://raw.githubusercontent.com/sanketpatel32/YumeDock/main/install.ps1 | iex`n``````"

$prereleaseArg = if ($isPrerelease) { "--prerelease" } else { "" }
gh release create $tagName $exe $zipPath --title "YumeDock $version" --notes $notes $prereleaseArg
if ($LASTEXITCODE -ne 0) { Fail "gh release create failed" }

Write-Host ""
Write-Host "  Done. Release published:" -ForegroundColor Green
Write-Host "  https://github.com/sanketpatel32/YumeDock/releases/tag/$tagName" -ForegroundColor Green
if ($Latest) {
    Write-Host "  install.ps1 now serves this build." -ForegroundColor DarkGray
}
Write-Host ""
