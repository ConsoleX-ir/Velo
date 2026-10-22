# ConsoleX Velo

ConsoleX Velo is a Windows-first desktop download manager. The UI is a Tauri 2 webview built with React and TypeScript; the native Rust application service calls the existing independent Rust download engine.

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

The native Tauri crate depends on the root `consolex-download-engine` library by path, keeping engine tests independent of Tauri and the webview.

## Current product limits

Downloads and terminal history are in memory for the current application session. HTTP and HTTPS are supported. Pause/resume, durable history/recovery, user-controlled retries, segmented transfers, categories, and file-opening actions are not available. The engine may continue an interrupted body using its bounded, validator-checked Range behavior; this is not durable resume.

See [the architecture](docs/ARCHITECTURE.md), [security requirements](docs/SECURITY_AND_PRIVACY.md), and [roadmap](docs/ROADMAP.md) for boundaries and remaining release work.
