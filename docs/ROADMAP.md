# Roadmap

**Status:** Proposed sequence after Phase 0  
**Research checked:** 2026-10-21

## Phase 0 — Specifications and plan (current)

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

1. Confirm Windows support target, minimum OS, x64/ARM64 scope, overwrite behavior, parent-directory behavior, timeout values, retry budget, TLS backend, and file durability policy. Record any changed decisions in ADRs.
2. Inspect current toolchains and choose crate/workspace layout. Create only the Rust core and test harness needed for library and integration testing; do not add the UI. Select Reqwest/Tokio features deliberately and commit the lockfile.
3. Define public request, download ID, progress snapshot, terminal result, typed errors, and the allowed state-transition table. Add unit tests for legal/illegal transitions and error mapping.
4. Implement URL and request validation: parse URL, allow only HTTP/HTTPS, reject embedded credentials and malformed/unsupported inputs, and set explicit request/redirect policy.
5. Implement destination policy and safe filename helpers. Validate parent and target, enforce no-clobber default, exclusively reserve a random same-directory temporary file, and cover Windows reserved names/collisions/reparse-point decisions with tests.
6. Implement an in-memory job registry and single-job task ownership. Reject duplicate IDs and conflicting destinations; keep engine state authoritative and terminal results reliable.
7. Build the reusable Reqwest client with bounded connection/overall timeouts and minimal features. Add explicit bounded redirects, downgrade rejection, and cross-origin sensitive-header rules (Phase 1 sends no user credentials or custom sensitive headers).
8. Implement response acceptance before file writes: status policy, length/encoding metadata, bounded safe diagnostics, and response-body rejection for unsupported partial responses.
9. Implement the one-stream transfer loop with bounded chunk memory, partial-write-safe accounting, checked `u64` counters, EOF/Content-Length validation, and no success promotion on mismatch.
10. Implement coalesced progress snapshots: immediate state transitions, at most one update per 100 ms or configured byte delta, bounded latest-value delivery, and guaranteed terminal result delivery.
11. Add cooperative cancellation at queued, request, stream, backoff, and finalization boundaries. Resolve cancellation versus promotion semantics, close file handles before cleanup, and report cleanup failures.
12. Add bounded retry classification and exponential backoff with jitter/Retry-After handling. Inject clock and jitter; reset partial output at byte zero for every retry. Unit-test every error class and attempt boundary.
13. Implement finalization: flush and chosen sync policy, re-check collision, promote temp file on the same volume, and set `Completed` only after promotion. Test existing destination preservation and Windows rename behavior.
14. Add local scripted HTTP integration tests for success, empty/unknown-length bodies, redirects, status failures, truncation/reset, retries, progress backpressure, and cancellation at phase boundaries; assert exact bytes and state sequences.
15. Add Windows-specific filesystem tests and run formatting, lint, unit, integration, and Windows CI checks. Fix failures before benchmarking; record exact commands/results.
16. Measure baseline performance and release-relevant metrics using `PERFORMANCE_PLAN.md`; set budgets only after evidence and product review.
17. Review security controls, dependency tree, error/log redaction, standard-user filesystem behavior, and the full acceptance matrix. Exit Phase 1 only when correctness gates pass and limitations are stated.

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
- Add user/global/host concurrency limits and adaptive behavior only after benchmarks and clear resource budgets.

## Phase 5 — Release hardening and ecosystem integrations

- Installer signing, upgrades/uninstall, update trust and rollback, accessibility/localization, performance budgets, privacy review.
- Browser integration only with an explicit permission model and supported browsers.
- BitTorrent is a separate protocol/subsystem decision and remains out of scope until a distinct product/security plan exists.

## Phase gates

Each phase starts only after the previous exit criteria and contracts are reviewed. Changes to stack, persistence guarantees, network permissions, telemetry, or overwrite semantics require a documented decision record. Roadmap dates are intentionally omitted until scope, staffing, and release targets are known.
