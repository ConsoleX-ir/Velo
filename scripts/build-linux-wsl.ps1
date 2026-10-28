# ConsoleX Velo — WSL Linux installer build launcher
#
# Run from Windows PowerShell. This script:
#   1. Copies the project into the WSL native filesystem for fast I/O
#   2. Installs missing system prerequisites inside WSL
#   3. Runs the existing build-linux-installer.sh inside WSL
#   4. Copies the resulting .deb and .AppImage back to the Windows project
#
# Node.js 22.12+ (or 24.x/26+) and npm must already be installed in the distro.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\build-linux-wsl.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\build-linux-wsl.ps1 -SkipSetup
#   powershell -ExecutionPolicy Bypass -File scripts\build-linux-wsl.ps1 -Distro ubuntu-22.04

param(
    [switch]$SkipSetup,
    [string]$Distro = ""
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$projectName = "consolex-velo"
$wslBuildDir = "/tmp/$projectName-build"

# ── WSL helper ──────────────────────────────────────────────────────────
function Invoke-WSL {
    param([string]$Command)
    if ($Distro) {
        wsl -d $Distro -- bash -lc $Command
    } else {
        wsl -- bash -lc $Command
    }
    if ($LASTEXITCODE -ne 0) { throw "WSL command failed: $Command" }
}

function Invoke-WSLRoot {
    param([string]$Command)
    if ($Distro) {
        wsl -d $Distro -u root -- bash -lc $Command
    } else {
        wsl -u root -- bash -lc $Command
    }
    if ($LASTEXITCODE -ne 0) { throw "WSL root command failed: $Command" }
}

# ── 1. Check WSL availability ──────────────────────────────────────────
Write-Host "`n== ConsoleX Velo WSL build ==" -ForegroundColor Cyan

$wslCheck = Get-Command wsl -ErrorAction SilentlyContinue
if (-not $wslCheck) {
    throw "WSL is not installed or not on PATH. Install it with: wsl --install"
}

$wslDistroLabel = if ($Distro) { $Distro } else { "default" }
Write-Host "WSL distro: $wslDistroLabel"

# ── 2. Install prerequisites (skip with -SkipSetup) ───────────────────
if (-not $SkipSetup) {
    Write-Host "`n-- Installing system prerequisites in WSL" -ForegroundColor Yellow
    Invoke-WSLRoot @"
apt-get update -qq &&
apt-get install -y -qq \
  libwebkit2gtk-4.1-dev \
  libgtk-3-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  libsoup-3.0-dev \
  libssl-dev \
  libxdo-dev \
  patchelf \
  pkg-config \
  build-essential \
  curl \
  wget \
  file
"@
    Write-Host "Prerequisites installed." -ForegroundColor Green
} else {
    Write-Host "`n-- Skipping system setup (-SkipSetup)" -ForegroundColor DarkGray
}

# ── 3. Copy project to WSL native filesystem ──────────────────────────
Write-Host "`n-- Copying project to WSL native filesystem ($wslBuildDir)" -ForegroundColor Yellow
Invoke-WSL "rm -rf $wslBuildDir && mkdir -p $wslBuildDir"

# rsync is faster and handles the /mnt/ overhead better than cp
Invoke-WSL @"
if command -v rsync &>/dev/null; then
  rsync -a --exclude=node_modules --exclude=target --exclude=.git '/mnt/d/ConsoleX/ConsoleX Velo/' $wslBuildDir/
else
  cp -r '/mnt/d/ConsoleX/ConsoleX Velo/'* $wslBuildDir/
  cp -r '/mnt/d/ConsoleX/ConsoleX Velo/'.git $wslBuildDir/ 2>/dev/null || true
fi
"@
Write-Host "Project copied." -ForegroundColor Green

# ── 4. Run the build inside WSL ───────────────────────────────────────
Write-Host "`n-- Running Linux build inside WSL" -ForegroundColor Yellow
Invoke-WSL "cd $wslBuildDir && bash scripts/build-linux-installer.sh"
Write-Host "Build complete." -ForegroundColor Green

# ── 5. Copy artifacts back to Windows ─────────────────────────────────
$winBundleDir = Join-Path $root "src-tauri\target\release\bundle"
Write-Host "`n-- Copying installers back to Windows" -ForegroundColor Yellow

# Create the destination directories
foreach ($fmt in @("appimage", "deb")) {
    $dest = Join-Path $winBundleDir $fmt
    if (-not (Test-Path $dest)) { New-Item -ItemType Directory -Path $dest -Force | Out-Null }
}

# Copy AppImage and .deb files back
Invoke-WSL @"
find $wslBuildDir/src-tauri/target/release/bundle \( -name '*.AppImage' -o -name '*.deb' \) -print0 |
  while IFS= read -r -d '' f; do
    sub=''; case "'"$f"'" in *.AppImage) sub=appimage ;; *.deb) sub=deb ;; esac
    dest='/mnt/d/ConsoleX/ConsoleX Velo/src-tauri/target/release/bundle/'"'"'$sub'"'"'/'
    cp -v "'"$f"'"' "'"$dest"'"'
  done
"@

# ── 6. Report results ─────────────────────────────────────────────────
Write-Host "`n== Done ==" -ForegroundColor Green
Write-Host ""
Write-Host "Installers produced:"

$artifacts = Get-ChildItem -Path $winBundleDir -Recurse -Include "*.AppImage", "*.deb" -ErrorAction SilentlyContinue |
    Sort-Object Name

if ($artifacts) {
    foreach ($a in $artifacts) {
        $sizeMB = [math]::Round($a.Length / 1MB, 1)
        Write-Host "  $($a.FullName)  ($sizeMB MB)"
    }
} else {
    Write-Host "  (no installers found — check the WSL build output above for errors)" -ForegroundColor Red
}

Write-Host ""
Write-Host "Cleanup: WSL build dir at $wslBuildDir (auto-cleaned on reboot). Remove manually with:"
Write-Host "  wsl -e bash -c 'rm -rf $wslBuildDir'"
