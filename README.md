# ConsoleX Velo

ConsoleX Velo is a Windows-first desktop download manager. The UI is a Tauri 2 webview built with React and TypeScript; the native Rust application service calls the existing independent Rust download engine. Velo runs as a real desktop application: an integrated custom title bar, a system tray with background execution, and a persistent default download folder.

## ScreenShot

![Velo Main Window](ScreenShot/Velo.png)

## Highlights

- **Downloads that keep working.** HTTP/HTTPS with bounded concurrency (4 active jobs), pause/resume, durable cross-restart resume from verified checkpoints, bounded automatic retry for transient network failures, and local history with DPAPI-protected URLs.
- **Custom window chrome.** The native title bar is replaced by an integrated one: drag, minimize, maximize/restore, and close all drive the real window. Closing the window hides Velo to the tray while transfers continue.
- **System tray.** Show/Hide/Exit menu, left-click restore, an explicit Exit that shuts the engine down in order, and a single-instance guard so a second launch focuses the running window instead of starting a second engine.
- **A default download folder that persists.** Chosen through the native folder dialog, validated (exists, is a directory, accepts writes) in Rust before being saved to `settings.json`, and shown with a recovery path if it ever disappears. Every download can override it without changing the default.
- **Honest, restrained interface.** Dark and light palettes, reduced-motion support, keyboard navigation with visible focus, a per-download context menu, and speed sparklines that only appear while a real rate is measured.

## Run locally

Prerequisites: Node.js/npm, Rust with the MSVC Windows target and Visual C++ Build Tools, and the WebView2 runtime/development prerequisites required by Tauri.

```powershell
npm install
npm run tauri dev
```

The frontend-only development server is available with `npm run dev`; native commands and download transfers require the Tauri application.

## Checks

```powershell
npm test
npm run build
cargo fmt --check
cargo check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings
```

The native Tauri crate depends on the root `consolex-download-engine` library by path, keeping engine tests independent of Tauri and the webview. (On a non-Windows development machine the `src-tauri` crate additionally needs the Tauri Linux system packages such as `libwebkit2gtk-4.1-dev`; on Windows with MSVC no extra system packages are required.)

## Build the Windows installer

From a Windows machine with the prerequisites above, run the packaging script or the equivalent commands:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-windows-installer.ps1
# or manually:
npm ci
npm run tauri build
```

The NSIS installer is written to `src-tauri\target\release\bundle\nsis\ConsoleX Velo_0.6.0_x64-setup.exe`. See [scripts/build-windows-installer.ps1](scripts/build-windows-installer.ps1) for prerequisite checks and options.

## Current product limits

HTTP and HTTPS are supported with queueing, bounded concurrency, pause/resume, durable local history (URLs DPAPI-protected), and user scheduled downloads that can be edited or cancelled and are restored after restart. Interrupted transfers resume from a verified checkpoint when the server supports strong validators; otherwise they restart from byte zero. Transient network failures use a bounded automatic retry schedule. Large files can use up to four concurrent, validator-pinned HTTP Range requests, with strict response checks and a safe single-stream fallback; segmented mode is bypassed while a speed cap is active. Persisted aggregate global and optional per-download speed caps are enforced by the engine. Categories, file-opening actions, and browser integration are not available.

See [the architecture](docs/ARCHITECTURE.md), [security requirements](docs/SECURITY_AND_PRIVACY.md), and [roadmap](docs/ROADMAP.md) for boundaries and remaining release work.
