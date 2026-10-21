# Roadmap

**Status:** Phase 0 documentation complete; Phase 1 not started
**Research checked:** 2026-10-21

## Phase 0 — Specifications and plan (completed)

- Inspect current workspace and record baseline.
- Finalize product scope, boundaries, architecture, download contract, performance protocol, security/privacy requirements, test strategy, ADRs, and implementation sequence.
- No production UI or download engine implementation.

## Phase 1 — Minimal reliable HTTP/HTTPS core

- Create Rust workspace/crate and pin a reviewed minimal dependency set.
- Implement request validation, safe destination/temp-file policy, typed state/error model, single-stream Reqwest transfer, progress coalescing, cancellation, bounded retry, length validation, finalization, and cleanup.
- Add local deterministic HTTP server and filesystem integration tests, including Windows path behaviors.
- Provide a narrow callable interface/test harness; no complete UI, SQLite recovery, resume, segmentation, adaptive concurrency, browser integration, or BitTorrent.
- Exit gate: required scenario matrix passes on Windows CI; no claims of resume or recovery.

### Phase 1 implementation sequence

1. Confirm minimum Windows version/filesystem support, TLS backend, system proxy behavior, and whether Windows reparse-point rejection can be implemented with handle-based checks. The Phase 1 contract is already no-clobber, existing-parent-only, `sync_all` before promotion, fixed provisional admission caps, and the provisional timeout/retry defaults in `DOWNLOAD_ENGINE_DESIGN.md`; changes require an ADR update.
2. Check toolchain availability and choose the single-crate layout. Create only the Rust core/test project; add a `.gitignore` for build output and commit the lockfile, but do not add UI or Tauri scaffold. Select Reqwest/Tokio features deliberately.
3. Define public request, download ID, progress snapshot, terminal result, typed errors, and the allowed state-transition table. Add unit tests for legal/illegal transitions and error mapping.
4. Implement URL and request validation: parse URL, allow only HTTP/HTTPS, reject embedded credentials and malformed/unsupported inputs, disable Referer forwarding, and set explicit request/redirect policy.
5. Implement destination policy and safe filename helpers. Require an existing canonical parent, reserve the case-insensitive Windows destination key in-process, reject existing targets, exclusively create a random same-directory temporary file, and cover Windows reserved names/collisions/reparse points with tests.
6. Implement an in-memory job registry and single-job task ownership. Enforce 4 active/100 retained-record provisional caps, reject duplicate IDs and conflicting destinations, keep engine state authoritative, and retain terminal results independently of progress until acknowledgement.
7. Build the reusable Reqwest client with provisional 10 s connect, 30 s read-inactivity, and 24 h network-job deadline policy. Use `default-features = false`, `stream`, `http2`, and one reviewed TLS backend (provisionally `rustls-tls`); do not enable transparent compression, cookies, or multipart/socks features. Disable Referer forwarding. Add explicit bounded redirects, downgrade rejection, and cross-origin sensitive-header rules (Phase 1 sends no user credentials/custom sensitive headers).
8. Implement response acceptance before file writes: final 200 only, identity/no content encoding, length metadata, bounded safe diagnostics, and rejection of other final statuses including 204/206.
9. Implement the one-stream transfer loop with bounded chunk memory, partial-write-safe accounting, checked `u64` counters, EOF/Content-Length validation, and no success promotion on mismatch.
10. Implement coalesced progress snapshots: immediate state transitions, no more than 10 ordinary updates per second per job (byte thresholds may suppress updates but never increase frequency), one latest-value slot, and a separately retained terminal result.
11. Add cooperative cancellation at queued, request, stream, backoff, and finalization boundaries. Resolve cancellation versus promotion semantics, close file handles before cleanup, and report cleanup failures.
12. Add bounded retry classification and exponential backoff with jitter/Retry-After handling. Inject clock and jitter; reset partial output at byte zero for every retry. Unit-test every error class and attempt boundary.
13. Implement finalization: `sync_all`, atomically promote on the same volume without replace-existing or copy-across-volume behavior, and set `Completed` only after promotion. Rust `std::fs::rename` may replace an existing target; use a narrow target-specific Windows API dependency if needed to guarantee no-clobber. Test external destination creation and preservation on Windows.
14. Add local scripted HTTP integration tests for success, empty/unknown-length bodies, redirects, status failures, truncation/reset, retries, progress backpressure, and cancellation at phase boundaries; assert exact bytes and state sequences.
15. Add Windows-specific filesystem tests and a minimal Windows CI workflow, then run formatting, lint, unit, integration, and CI checks. Fix failures before benchmarking; record exact commands/results. Use Tokio `fs`, `io-util`, `sync`, `time`, and test-only `net`/`macros`/runtime features as required by actual code; avoid a separate HTTP test-server crate by using a scripted loopback server over Tokio if practical. `tempfile` is a reasonable dev-only dependency for isolated directories; justify or replace it with a reviewed std-only helper. `windows-sys` is justified as a target-specific dependency for the no-replace Windows promotion and, if chosen, reparse-point inspection. Use manual typed error implementations unless a small error-derive crate materially reduces boilerplate.
16. Measure baseline performance and release-relevant metrics using `PERFORMANCE_PLAN.md`; set budgets only after evidence and product review.
17. Review security controls, dependency tree, error/log redaction, standard-user filesystem behavior, and the full acceptance matrix. Exit Phase 1 only when correctness gates pass and limitations are stated.

### Recommended first implementation increment

The first change should be a standalone Rust library crate (no Tauri or frontend) containing only the `DownloadId`, request/config DTOs, lifecycle enum/transition validator, typed terminal/error model, and pure URL validation. Its only runtime dependency should be `url` for standards-based parsing; write the small typed errors without an error-derive dependency. Add unit tests for accepted HTTP/HTTPS URLs and rejected unsupported schemes, malformed URLs, and embedded credentials, plus table-driven legal/illegal state transitions. Acceptance is `cargo fmt --check`, `cargo test`, and `cargo check --target x86_64-pc-windows-msvc` with the selected pinned toolchain; tests must prove validation fails before any network or filesystem side effect. Keep this increment free of Reqwest, Tokio runtime, SQLite, UI, and filesystem behavior; add network/filesystem dependencies with the next reviewed increments. Then continue with destination reservation/temp cleanup before integrating the network stream.

## Phase 2 — Desktop shell and minimal user workflow

- Establish Tauri 2 + React/TypeScript shell, strict CSP/capabilities, narrow IPC commands, accessible minimal add/download/status/cancel workflow.
- Connect UI to tested engine; test event ordering/backpressure and packaged standard-user behavior.
- Decide exact Windows support matrix, installer type, WebView2 strategy, app identity, signing, and distribution.

## Phase 3 — Durable metadata and startup recovery

- Add SQLite in per-user application data with versioned migrations and transaction policy.
- Persist user intent and terminal outcomes; model process interruption honestly.
- Design recovery for partial files without claiming resumability until remote validator/range rules are implemented. Test crash points and database/sidecar handling.

## Phase 4 — Resume and concurrency

- Specify ETag/Last-Modified/Range behavior, validators, server incompatibilities, atomic assembly, retry semantics, and integrity expectations.
- Implement reliable single-stream resume first, then consider segmented downloads behind measured evidence.
- Revisit user/global/host concurrency limits and adaptive behavior only after benchmarks and clear resource budgets; Phase 1 already has a fixed provisional admission cap.

## Phase 5 — Release hardening and ecosystem integrations

- Installer signing, upgrades/uninstall, update trust and rollback, accessibility/localization, performance budgets, privacy review.
- Browser integration only with an explicit permission model and supported browsers.
- BitTorrent is a separate protocol/subsystem decision and remains out of scope until a distinct product/security plan exists.

## Phase gates

Each phase starts only after the previous exit criteria and contracts are reviewed. Changes to stack, persistence guarantees, network permissions, telemetry, or overwrite semantics require a documented decision record. Roadmap dates are intentionally omitted until scope, staffing, and release targets are known.
