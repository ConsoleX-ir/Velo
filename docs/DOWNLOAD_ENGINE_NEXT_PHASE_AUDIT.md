# Download Engine: Next-Phase Audit

**Audit basis:** `main` at `886cde7605e9dd47d133b0344c07ca070d26334f` (2026-10-24, v0.5 design commit)  
**Scope:** Rust download engine and Tauri service; UI redesign excluded.  
**Purpose:** Record implementation order and acceptance gates before expanding concurrency or adding new transfer modes.

## Executive decision

Do not jump straight to a high default concurrency or implement segmented downloading and scheduling in one patch. The existing engine already has FIFO admission, bounded active jobs, retained terminal results, validator-checked Range continuation, and same-session pause/resume. The safest next step is to make concurrency a validated application setting, prove behavior under 10/16/24-job workloads, and decouple queue size from retained terminal-result capacity. Scheduling and segmented downloading should follow as separate, independently testable changes.

## Confirmed implementation facts

- The engine is explicitly documented in `src/lib.rs` as a managed **single-stream** HTTP/HTTPS core.
- `DownloadManagerConfig` has `max_active` and `max_retained_jobs`; its defaults are 4 and 100.
- The core supports FIFO scheduling, cooperative cancellation, coalesced progress snapshots, retained terminal results, bounded Range continuation after interrupted bodies, and strong-ETag same-session pause/resume.
- The Tauri service (`src-tauri/src/service.rs`) owns application history, retry behavior, and durable resume metadata. That makes it the correct integration layer for user-facing settings and scheduled-job persistence, while transfer correctness should remain in the engine.
- Range continuation after a broken body is **not** segmented downloading: it continues a single transfer from a verified offset; it does not split one file into simultaneous byte ranges.
- The existing performance plan (`docs/PERFORMANCE_PLAN.md`) records four concurrent transfers and eight queued jobs under a four-active cap. It does not establish resource behavior at 10, 16, or 24 active transfers. Existing timings are explicitly exploratory and do not include CPU/RAM sampling.

## Prioritized implementation plan

### Phase A — Concurrency and admission (first code change)

1. Introduce a typed, persisted maximum-active-download setting with a conservative validated range. Start with 1–10; allow increasing the upper bound only after resource measurements.
2. Ensure the setting reaches the actual `DownloadManager` construction path, not just the core config type. Validate values in Rust regardless of frontend validation.
3. Keep active concurrency separate from total queued jobs and retained terminal records. Define explicit caps for queued jobs and history; do not use the engine's terminal-result retention limit as the user-visible history limit.
4. Add regression tests for 1, 4, and 10 active jobs, FIFO admission, cancellation while queued, changing the limit while jobs are active, and terminal acknowledgement/retention.
5. Preserve current defaults until benchmarks show that a different default is better.

**Acceptance:** observed peak active requests never exceed the configured limit; all queued jobs reach a deterministic terminal or cancelled state; no duplicate destination writes; no unbounded progress/event growth; existing pause/resume/retry and restart recovery tests pass.

### Phase B — High-volume queue and history lifecycle

1. Support a large persisted queue without requiring every queued record to be an active engine handle.
2. Separate queue persistence, live engine registration, and completed-history retention.
3. Use stable IDs and idempotent startup recovery; reconcile records with missing/corrupt temp files and existing destination files without silently overwriting anything.
4. Add stress tests at 100, 1,000, and 10,000 queued metadata records; measure memory and load/save latency before selecting supported product limits.
5. Keep URL secrets redacted in UI/logs and preserve the existing local-at-rest protections.

**Acceptance:** bounded memory under large queues; startup recovery is deterministic; corrupt records do not erase unrelated history; deleting history does not accidentally cancel active transfers unless explicitly specified.

### Phase C — Scheduled downloads

1. Represent a scheduled job as persisted metadata with a UTC instant, original time-zone context for display, status, and stable ID.
2. Store schedules separately from live engine jobs; do not hold an active transfer slot while waiting for its scheduled time.
3. Reconcile due jobs on startup, define behavior after sleep/offline periods, and make dispatch idempotent so a job cannot start twice.
4. Use a single scheduler service with bounded wakeups rather than one sleeping task per queued item.
5. Test clock changes, daylight-saving transitions, app restart, duplicate due events, cancellation/editing, and invalid/past dates.

**Acceptance:** scheduled jobs survive restart, start at most once, do not consume active slots before dispatch, and transition predictably when Velo was closed at the scheduled time. Document that downloads cannot start while the app is not running unless a separate Windows background service is deliberately implemented.

### Phase D — True segmented downloads

1. Probe server support safely (known content length, byte-range support, stable validators) and fall back to single-stream when preconditions are absent.
2. Split a file into bounded ranges only when the server's representation can be verified consistently; use per-segment temporary state and strict Content-Range/body-length validation.
3. Never append data based only on a server claiming to support ranges. Verify validators across all segments and reject mismatched responses.
4. Assemble in deterministic byte order into a sibling temporary output, verify total length, then promote atomically without clobbering.
5. Bound global and per-file concurrency to avoid multiplying active transfers unexpectedly. Retry only failed segments under strict budgets; cancellation and pause must stop all workers.
6. Add hostile local-server tests: ignored Range, wrong offsets, overlapping/gapped ranges, changed ETag, truncated segment, redirects, malformed lengths, cancellation during assembly, and disk-full failures.

**Acceptance:** byte-for-byte or cryptographic-hash match against the source fixture; no final output on partial failure; no existing destination overwritten; resource limits hold globally and per file; fallback is automatic and safe.

### Phase E — Benchmark and release gate

Extend `docs/PERFORMANCE_PLAN.md` with controlled cases for 1/4/10/16 active transfers and 100/1,000/10,000 queued records. Capture median and p90 across repeated runs, process CPU/private memory, event rate, throughput, queue latency, and disk use. Use loopback fixtures first, then a controlled LAN. Do not infer throughput gains from the number of connections alone.

## Security and reliability gates

The existing `docs/SECURITY_AND_PRIVACY.md` correctly leaves several Windows filesystem and URL policy issues open. Before adding browser ingestion or background URL sources, resolve local/private-network URL policy. Before release, test Windows reparse-point/junction behavior and no-clobber promotion under races, redirect credential isolation, URL/query redaction, and packaged WebView2/CSP behavior. A requirements document is not evidence that these gates have passed.

## Documentation drift to reconcile

- `docs/PRODUCT_SCOPE.md` still describes an early Phase 0/Phase 1 scope and appears to exclude pause/resume, durable checkpoints, retries, and scheduling even though the current implementation has pause/resume, checkpointed resume, and bounded retries. Update it after confirming the current command/API surface.
- Installer/version references should be checked against the current `0.5.0` package metadata before a release; do not assume a script path or installer name has kept pace with version changes.
- Keep benchmark claims explicitly tied to commit, environment, and test setup; the current performance document already notes its historical run is exploratory.

## Immediate next pull request after this audit

Implement Phase A only: wire a validated concurrency setting through Rust service construction, keep the existing default until benchmark evidence supports changing it, and add engine/service tests for limits and queue lifecycle. Do not combine scheduling, segmentation, and UI changes into that PR.
