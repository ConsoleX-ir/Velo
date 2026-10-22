# Roadmap

**Status:** Phase 0 complete; Phase 1A and Phase 1B implemented in the Rust core
**Research checked:** 2026-10-21

## Phase 0 — Specifications and plan (completed)

- Inspect current workspace and record baseline.
- Finalize product scope, boundaries, architecture, download contract, performance protocol, security/privacy requirements, test strategy, ADRs, and implementation sequence.
- No production UI or download engine implementation.

## Phase 1 — Minimal reliable HTTP/HTTPS core

### Phase 1A — Single-transfer foundation (implemented)

- Existing Rust library with Reqwest streaming, HTTP/HTTPS validation, redirect limits and HTTPS downgrade prevention, timeouts, response-length checks, exclusive sibling temp files, and no-clobber promotion.
- Local scripted HTTP and filesystem tests; no durable checkpoint, pause/resume, segmented-transfer, or crash-recovery claims.

### Phase 1B — Managed jobs and safe continuation (implemented)

- Shared in-memory manager with synchronous atomic admission, caller IDs plus generation-bearing handles, FIFO scheduling, default four active jobs and 100 retained records, coalesced latest-value progress, cooperative cancellation, one terminal result per accepted job, terminal acknowledgement, worker-panic cleanup, and idempotent shutdown.
- Each job starts with one full GET and may use up to three strong-ETag-validated Range continuations after an interrupted response. Byte-zero retries, SQLite, crash recovery, user-controlled pause/resume, segmented downloads, adaptive concurrency, BitTorrent, and UI integration remain out of scope.
- Phase 1B validation (2026-10-22): On Windows, `cargo fmt --check`, `cargo check`, `cargo test` (62 passed, 0 failed), and `cargo clippy --all-targets --all-features -- -D warnings` passed. The Windows-specific promotion and case-insensitive reservation tests ran in that suite.

### Next milestones

1. **Phase 1C — Retry policy (deferred):** define classification, backoff/Retry-After, byte-zero restart, and deterministic timing tests before implementation.
2. **Phase 2 — Desktop shell:** Tauri 2 + React/TypeScript, narrow IPC commands, CSP/capabilities, and a minimal user workflow connected to the tested manager.
3. **Phase 3 — Durable metadata:** SQLite migrations and startup recovery with honest interrupted-job semantics.
4. **Phase 4 — Durable resume and transfer concurrency:** define checkpoint and restart-recovery semantics; consider segmentation only after correctness and measurement.
5. **Phase 5 — Release hardening and integrations:** signing, upgrades, accessibility, performance budgets, browser integration, and a separate BitTorrent security/product decision.

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
