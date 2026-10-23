# Roadmap

**Status:** Phase 0, Phase 1A/1B, Phase 2 desktop application, and the Phase X hardening pass (durable cross-restart resume, hardened JSON persistence, bounded auto-retry, UI component system) implemented; Windows target validation pending
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

1. **Phase 1C — Retry policy (implemented in the Phase X pass):** bounded automatic retry (max 2 attempts, injectable 2s/8s backoff) for transient network-failure classes and HTTP 5xx only; user cancellation and permanent failures are never retried automatically; manual retry is preserved. Retry-After handling remains a future refinement.
2. **Phase 2 — Desktop shell (implementation present; close after target validation):** Tauri 2 + React/TypeScript shell, narrow IPC commands, CSP/capabilities, and a user workflow connected to the tested manager. Verify packaged Windows behavior and keyboard/accessibility behavior before release.
3. **Phase 3 — Durable metadata (implemented in the Phase X pass, without SQLite):** versioned JSON snapshot envelopes with atomic synced writes, corruption sidecars, truncated-record repair, and startup recovery with honest interrupted-job semantics; SQLite stays deferred unless scale or query needs demand it (per ADR 0003 and ADR 0006).
4. **Phase 4 — Durable resume and transfer concurrency (single-stream resume implemented in the Phase X pass):** persisted checkpoint/temp adoption with full file and server revalidation, documented byte-zero fallback; segmented downloads and adaptive concurrency remain behind measured evidence.
5. **Phase 5 — Release hardening and integrations:** signing, upgrades, accessibility, performance budgets, browser integration, and a separate BitTorrent security/product decision.

## Phase 2 — Desktop shell and minimal user workflow

- Establish Tauri 2 + React/TypeScript shell, strict CSP/capabilities, narrow IPC commands, accessible minimal add/download/status/cancel workflow.
- Connect UI to tested engine; test event ordering/backpressure and packaged standard-user behavior.
- Decide exact Windows support matrix, installer type, WebView2 strategy, app identity, signing, and distribution.

Implementation now present: local Tauri shell, responsive Aurora Stream UI rebuilt on a local dependency-free component set (dialog, tooltip, segmented control, switch, toast, state pill, progress, spinner, empty state) with keyboard accessibility and reduced-motion support, native save-dialog workflow, typed command DTOs, manager-backed list/start/cancel/pause/resume/retry/dismiss flow, theme and reduced-motion preferences, persisted local history with DPAPI-protected URLs, plus frontend and native service tests. Restarted in-progress jobs are marked Interrupted; manual retry re-verifies the persisted partial and checkpoint and continues from the verified offset when the server allows it, falling back to byte zero otherwise. Transient failures auto-retry on a bounded backoff schedule. Categories and file opening remain unsupported. Windows installer smoke testing and interactive GUI validation remain release gates.

## Phase 3 — Durable metadata and startup recovery (implemented without SQLite)

- Ship a versioned JSON snapshot envelope under per-user application data with atomic synced replacement, restrictive permissions, corruption sidecars, truncated-record repair, and a one-shot recovery notice in the UI (ADR 0006).
- Persist user intent, terminal outcomes, and durable-resume metadata; model process interruption honestly.
- Partial files are never trusted implicitly: adoption requires on-disk re-verification plus server revalidation before any append, with a documented byte-zero fallback.
- SQLite remains deferred unless filtering/query volume or concurrent writers outgrow the snapshot model.

## Phase 4 — Resume and concurrency

- Single-stream durable resume is implemented: persisted temp/checkpoint adoption validated on disk (same directory, regular file, length at or beyond the persisted counter, below the recorded total) and revalidated with the server (`If-Range`, `206`, matching `Content-Range`); every failure mode falls back to a documented byte-zero restart.
- Segmented downloads and adaptive concurrency remain future work and only behind measured evidence.
- Revisit user/global/host concurrency limits only after benchmarks and clear resource budgets; Phase 1 already has a fixed provisional admission cap.

## Phase 5 — Release hardening and ecosystem integrations

- Installer signing, upgrades/uninstall, update trust and rollback, accessibility/localization, performance budgets, privacy review.
- Browser integration only with an explicit permission model and supported browsers.
- BitTorrent is a separate protocol/subsystem decision and remains out of scope until a distinct product/security plan exists.

## Phase gates

Each phase starts only after the previous exit criteria and contracts are reviewed. Changes to stack, persistence guarantees, network permissions, telemetry, or overwrite semantics require a documented decision record. Roadmap dates are intentionally omitted until scope, staffing, and release targets are known.
