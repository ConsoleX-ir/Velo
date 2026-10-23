# ConsoleX Velo — Windows installer build script
#
# Builds the NSIS installer from this source tree. Run from a Windows machine
# (Windows 10/11 x64) with:
#   * Node.js 18+ and npm on PATH
#   * Rust stable with the MSVC toolchain (`rustup default stable`) — the
#     "Desktop development with C++" Visual Studio Build Tools workload
#   * WebView2 (preinstalled on Windows 11; Tauri offers the bootstrapper at
#     install time otherwise)
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\build-windows-installer.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\build-windows-installer.ps1 -SkipChecks

param(
    [switch]$SkipChecks
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

Write-Host "== ConsoleX Velo installer build ==" -ForegroundColor Cyan
Write-Host "Source tree: $root"

if (-not $SkipChecks) {
    Write-Host "`n-- Checking prerequisites" -ForegroundColor Yellow
    foreach ($tool in @("node", "npm", "cargo", "rustc")) {
        if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
            throw "$tool was not found on PATH. Install it and re-run."
        }
    }
    $rustVersion = (rustc --version)
    Write-Host "rustc: $rustVersion"
    Write-Host "node:  $(node --version)"

    # Verify the MSVC target that Tauri needs on Windows.
    $targets = rustup target list --installed
    if (-not ($targets -match "x86_64-pc-windows-msvc")) {
        Write-Host "Adding the x86_64-pc-windows-msvc target..."
        rustup target add x86_64-pc-windows-msvc
    }

    # Install frontend dependencies from the lockfile for a reproducible tree.
    Write-Host "`n-- Installing frontend dependencies (npm ci)" -ForegroundColor Yellow
    npm ci
    if ($LASTEXITCODE -ne 0) { throw "npm ci failed." }
}

# Frontend production build + type check (runs inside tauri build as well;
# doing it here surfaces errors early).
Write-Host "`n-- Building frontend (tsc + vite)" -ForegroundColor Yellow
npm run build
if ($LASTEXITCODE -ne 0) { throw "frontend build failed." }

# Native checks: engine crate (root) and Tauri app crate (src-tauri).
Write-Host "`n-- Running Rust checks" -ForegroundColor Yellow
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml

# The actual bundling: release build + NSIS installer.
Write-Host "`n-- Building the NSIS installer (npm run tauri build)" -ForegroundColor Yellow
npm run tauri build
if ($LASTEXITCODE -ne 0) { throw "tauri build failed." }

$bundle = Get-ChildItem -Path "src-tauri\target\release\bundle\nsis" -Filter "*.exe" |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $bundle) { throw "No installer was produced under src-tauri\target\release\bundle\nsis." }

Write-Host "`n== Done ==" -ForegroundColor Green
Write-Host "Installer: $($bundle.FullName) ($([math]::Round($bundle.Length / 1MB, 1)) MB)"
Write-Host "Smoke-test it by installing, launching, adding a download, closing the window"
Write-Host "(Velo should stay in the tray and keep transferring), and exiting from the tray."
