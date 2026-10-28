#!/usr/bin/env bash
# ConsoleX Velo — Linux installer build script
#
# Builds AppImage and .deb installers from this source tree.
# Run from a Linux machine (Ubuntu 22.04+ / Debian 12+ or equivalent)
# with the following installed:
#
#   * Node.js 22.12+ (or 24+, 26+) and npm
#   * Rust stable toolchain (rustup default stable)
#   * System libraries: libwebkit2gtk-4.1-dev, libgtk-3-dev,
#     libayatana-appindicator3-dev (tray), librsvg2-dev,
#     libssl-dev, pkg-config, build-essential
#
# Usage:
#   bash scripts/build-linux-installer.sh
#   bash scripts/build-linux-installer.sh --skip-checks

set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

# Source Rust environment if cargo isn't on PATH (common in non-interactive shells).
if ! command -v cargo &>/dev/null; then
  for env_file in "$HOME/.cargo/env" "$HOME/.rustup/env"; do
    if [[ -f "$env_file" ]]; then
      # shellcheck source=/dev/null
      source "$env_file"
      break
    fi
  done
fi

SKIP_CHECKS=false
if [[ "${1:-}" == "--skip-checks" ]]; then
  SKIP_CHECKS=true
fi

echo "== ConsoleX Velo Linux installer build =="
echo "Source tree: $root"

if [[ "$SKIP_CHECKS" == false ]]; then
  echo ""
  echo "-- Checking prerequisites"

  for tool in node npm cargo rustc pkg-config; do
    if ! command -v "$tool" &>/dev/null; then
      echo "ERROR: $tool not found on PATH. Install it and re-run." >&2
      exit 1
    fi
  done

  echo "rustc: $(rustc --version)"
  node_version="$(node -p 'process.versions.node')"
  if ! node -e 'const [major, minor] = process.versions.node.split(".").map(Number); if (!((major === 22 && minor >= 12) || major === 24 || major >= 26)) process.exit(1)'; then
    echo "ERROR: Node.js $node_version is unsupported by the locked frontend dependencies. Use 22.12+, 24.x, or 26+." >&2
    exit 1
  fi
  echo "node:  v$node_version"

  # Verify the default target is Linux x86_64 or aarch64.
  local_target="$(rustc -vV | grep host | cut -d' ' -f2)"
  echo "Rust target: $local_target"

  # Check for required system libraries.
  echo ""
  echo "-- Checking system libraries"
  for lib in webkit2gtk-4.1 gtk+-3.0 ayatana-appindicator3-0.1 librsvg-2.0 openssl; do
    if ! pkg-config --exists "$lib" 2>/dev/null; then
      echo "WARNING: $lib not found via pkg-config. Build may fail."
      echo "  On Ubuntu/Debian: sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libssl-dev"
    fi
  done

  # Install frontend dependencies.
  echo ""
  echo "-- Installing frontend dependencies (npm ci)"
  npm ci
fi

# Frontend production build + type check.
echo ""
echo "-- Building frontend (tsc + vite)"
npm run build

# Rust checks on both crates.
echo ""
echo "-- Running Rust checks"
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test

cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml

# Build Tauri app — produces AppImage + .deb in src-tauri/target/release/bundle/
echo ""
echo "-- Building Linux installers (npm run tauri build)"
npm run tauri build

# Report results.
echo ""
echo "== Done =="
echo ""
echo "Installers produced:"

for bundle_dir in appimage deb; do
  dir="src-tauri/target/release/bundle/$bundle_dir"
  if [[ -d "$dir" ]]; then
    find "$dir" -maxdepth 1 -type f \( -name '*.AppImage' -o -name '*.deb' \) | while read -r f; do
      size=$(du -h "$f" | cut -f1)
      echo "  $f  ($size)"
    done
  fi
done

echo ""
echo "Smoke-test:"
echo "  AppImage:  chmod +x <file>.AppImage && ./<file>.AppImage"
echo "  deb:       sudo dpkg -i <file>.deb && consolex-velo"
