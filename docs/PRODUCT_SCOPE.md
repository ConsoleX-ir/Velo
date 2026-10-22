# ConsoleX Velo Product Scope

**Status:** Phase 0 baseline  
**Research checked:** 2026-10-21

## Product intent

ConsoleX Velo is a Windows-first, free, ad-free desktop download manager. It should make ordinary HTTP/HTTPS downloads predictable, safe to control, and understandable when they fail. It has no mandatory account, cloud service, AI feature, ads, or telemetry.

## Phase 1 product boundary

Phase 1 is a reliable native HTTP/HTTPS download core and its automated tests. The core accepts one URL and one user-selected destination, streams bytes to disk, reports bounded progress, supports cancellation, and returns typed outcomes. A small test harness or command-line test surface is acceptable if required to exercise the library; it is not the product UI.

Included:

- HTTP and HTTPS GET for valid `http`/`https` URLs.
- Destination validation, collision-safe output creation, safe filename derivation when the caller requests a suggested name, and explicit error reporting.
- Phase 1 is no-clobber only: an existing destination or a finalization-time collision is an error. There is no overwrite flag; a later overwrite feature needs its own explicit semantics.
- Bounded redirect handling, response validation, streaming writes, byte accounting, cancellation, and strict in-job Range continuation after interrupted bodies when a known length and strong ETag permit safe continuation.
- Deterministic integration tests against an in-process/local HTTP server.

Excluded from Phase 1: complete UI, browser integration, BitTorrent, segmented or parallel-range downloads, adaptive concurrency, SQLite recovery, user-controlled pause/resume, durable checkpoints, byte-zero retries, scheduling, automatic update service, and cloud sync.

## Product principles and constraints

- Windows is the first release platform; the stack decision is Tauri 2, Rust, React + TypeScript, Tokio, Reqwest, and SQLite for persistent metadata.
- Prefer local-only behavior. No network calls beyond the user-requested download and explicitly user-initiated update checks if introduced in a later phase.
- Show progress based on bytes successfully written, not bytes merely received from the network.
- Describe the implemented behavior narrowly: an interrupted live transfer may continue through a validator-checked Range request. Do not imply user-controlled pause/resume, process-restart recovery, or integrity verification.
- Errors should identify the failed operation and offer a useful remedy without exposing credentials or unnecessarily echoing sensitive URL data.
- A proposed Windows installer size of approximately 100–130 MB is a planning goal only. Installer and installed sizes require measurement on release builds and are sensitive to WebView2 packaging choices.

## Release assumptions to validate

1. Initial distribution is a direct Windows download, likely an NSIS per-user installer; final choice depends on signing, update, and user-install requirements.
2. Windows 10/11 x64 is the initial support target; exact minimum Windows version and ARM64 support remain open.
3. Users choose their download destination. The application should not elevate privileges for ordinary downloads.
4. Initial functionality is single-stream per download, with provisional limits of 4 active jobs and 100 retained job records, including unacknowledged terminal records. Adaptive concurrency is deferred.

## Success criteria (to be measured, not asserted)

- Correct output bytes for known fixtures and successful downloads.
- Safe, deterministic cancellation and cleanup outcomes.
- Clear distinction among HTTP status failures, transport failures, local I/O failures, cancellation, and incomplete bodies.
- Stable memory use bounded by stream/chunk and channel buffers rather than total object size.
- Installer size, installed footprint, startup time, idle/active memory, CPU, throughput, and progress overhead recorded with the release benchmark procedure in `PERFORMANCE_PLAN.md`.

## Unresolved product questions

- Exact supported Windows versions and architectures.
- Installer format, per-user versus per-machine install, signing identity, and WebView2 runtime distribution.
- Whether a later phase should offer an explicit overwrite workflow; Phase 1 does not overwrite.
- Whether HTTP URLs are allowed, and whether HTTPS-to-HTTP redirects are rejected (Phase 1 decision: reject downgrade redirects).
- Product name, publisher identity, icon, localization, and accessibility requirements.
- Whether manually entered URLs may target loopback/private-network addresses; decide before browser integration or background URL ingestion.
- Later-phase durable pause/resume and restart-recovery policy, including checkpoint storage and verification of partial files.
