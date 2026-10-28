# Test Strategy

**Status:** Engine, UI/IPC, pause/resume, durable cross-restart resume, hardened persistence, and bounded-retry coverage; Windows engine validation passed 65 tests on 2026-10-22; Phase X pass adds engine (71), service-harness (14), and frontend (21) suites on Linux 2026-10-23
**Research checked:** 2026-10-22

## Phase 1 test layers

1. **Unit tests:** URL validation, response-header validation, partial-write accounting, cleanup error composition, progress coalescing, generation checks, manager lifecycle, pause/resume checkpoint validation, and local history snapshot replacement.
2. **Engine integration tests:** use a loopback ephemeral-port HTTP server with scripted responses; no public internet. Compare output bytes directly so no hash dependency is needed for the fixture gate.
3. **Filesystem integration tests:** temporary directories; exact bytes, collision behavior, failure cleanup, existing-file preservation, no-replace promotion, and finalization behavior.
4. **Windows-specific tests:** reserved/device names, Unicode and long paths where supported, ACL/permission failure, junction/reparse-point cases, concurrent destination reservation, installer smoke test when packaging exists.
5. **Frontend/IPC tests:** model tests cover revision-based merging, filtering, formatting, URL/name validation, bounded speed-history sampling, and state semantics; app tests cover schedule and speed preference controls alongside pause/resume/retry, stale snapshots, chart rendering, terminal metrics, keyboard behavior, command failures, and history notices. Tauri service tests cover scheduling edits/cancellation, schedule-history persistence, speed-setting persistence and v1 settings migration, as well as recovery, retry, and corrupt-history cases. DPAPI, packaged WebView behavior, and installer checks remain Windows release checks.

## Required scenario matrix

| Area | Cases | Required assertions |
|---|---|---|
| Validation | bad URL, unsupported scheme, embedded credentials, invalid destination | no network request or final file created; typed error |
| Success | empty body, known body, unknown length, chunked body | exact output bytes; byte count equals bytes written; terminal state order |
| Status | final 200, 204, 206, other 2xx, 3xx, 4xx, 5xx | only 200 accepted; no redirect body saved; no false completion |
| Redirect | valid same-origin, cross-origin, loop, over limit, HTTPS downgrade | bounded hops; policy enforced; no sensitive header leakage |
| Incomplete | declared larger/smaller, connection reset mid-body | fail; do not promote; accurate partial-byte/cleanup result |
| Cancellation | queued, connect, headers, body, backoff, race with finalization | accepted cancel yields Cancelled only after cleanup; cancel after Finalizing returns TooLate; exactly one terminal outcome |
| Pause/resume | queued pause, body-boundary pause, resume with valid ETag/total, resume after validator mismatch | same-session resume uses exact offset and If-Range; unsupported checkpoints safely restart from zero |
| Cross-restart resume | valid persisted partial+checkpoint, shrunk/missing/oversized partial, changed resource, 200-after-Range, two-worker adoption | partial adopted only after on-disk re-verification and server revalidation (206 + Content-Range start == real length + If-Range); adoption hands off to a process-owned temp name; every failure falls back to byte zero without appending |
| Persistence/recovery | snapshot replacement, malformed history, versioned envelope, truncated array, protected URL roundtrip, app restart with active/paused/completed rows | snapshots remain readable; unreadable snapshots are sidecar-backed-up and surfaced, never silently discarded; v1 arrays still load; URLs decrypt only for the protecting Windows account; unfinished jobs become Interrupted and restore checkpoint-continue or byte-zero messaging honestly; missing/size-changed completed files are flagged |
| Automatic retry | transient connect/timeout/inactivity/incomplete/5xx, permanent 4xx, user cancellation during backoff, retry budget exhaustion | only transient classes auto-retry with bounded injectable backoff (max 2); cancellation stops the timer and never produces a network retry; exhaustion surfaces a normal failure with manual retry preserved; error text never includes sensitive URLs |
| Managed jobs | duplicate IDs, destination conflicts, retained capacity, FIFO order, configured concurrency, acknowledgement, stale handles | admission is atomic; retained capacity includes queued and terminal-unacknowledged records; stale generations cannot act on reused IDs |
| Shutdown and worker failure | repeated/concurrent shutdown, queued cancellation, active cancellation, finalizing job, worker panic | admission closes once; no new dispatch after close; panic becomes one retained terminal failure; cleanup failure is preserved |
| Destination | existing file, two jobs same path, external target created before promotion, invalid/sanitized name, disk write failure, cleanup failure | no unintended clobber; exactly one same-process reservation winner; deterministic error and cleanup path |
| Progress | fast chunks, slow chunks, slow subscriber | state transitions immediate; ordinary updates ≤10/second/job; at most one pending snapshot/job; terminal result retained separately |

## Determinism and test hygiene

- Bind only loopback and use ephemeral ports. Ensure server tasks and temp directories are shut down in cleanup, including on test failure.
- Use deterministic server gates and state inspection instead of timing-sensitive sleeps where practical. Retry backoff timing is injectable, so scheduled-retry tests assert the schedule and cancellation semantics without real-time waits.
- Assert state transitions as an allowed sequence, not timing-sensitive exact timestamps.
- Avoid depending on public endpoints, DNS, external certificates, or uncontrolled bandwidth.
- Use fixed byte fixtures and exact byte comparisons; do not add a digest dependency solely for test convenience.
- Keep tests independent and parallel-safe with unique directories/ports.

## Measurable Phase 1 acceptance gates

- Compare output byte-for-byte with 0-byte, 1 MiB, and 64 MiB fixtures; all successful cases must have exact expected length when known.
- For a scripted sequence that ends in a terminal result, assert exactly one terminal result per accepted job. Rejected `start` requests are not jobs and receive a synchronous start error.
- Assert progress snapshots coalesce to the latest value, ordinary updates are at most 10 per second/job, and state transitions publish immediately. A slow subscriber cannot block transfer or lose a separately queryable terminal result.
- Single-stream jobs may make at most three in-job Range continuations after interruption. Separately test segmented probe validation, up to four concurrent disjoint ranges, exact response validators/lengths, ordered assembly, malformed/ignored Range fallback, cancellation cleanup, and exact output bytes. Verify the aggregate global cap across concurrent jobs and the optional per-job cap.
- Verify user schedules survive a fresh service instance, edit and cancel their timers, and submit through normal queue/retention limits when due. A scheduled job must not hold an active slot before its due time.
- For every failure/cancellation test, assert an existing final file is byte-identical and any undeleted temporary path is included in a `Failed` result.
- Reject all final statuses except 200; accept absent Content-Length only after successful EOF, and reject both short and long bodies when Content-Length is present.
- Assert non-200 response bodies are neither saved nor retained in diagnostics; a full response with `Content-Range` or non-identity content encoding is rejected.
- For close-delimited responses without Content-Length, assert the protocol EOF outcome and unknown total; do not claim the client can detect a peer that closed early at a syntactically valid boundary.
- Verify in code review and stress test that response bodies are streamed and there is no file-size-proportional buffer or unbounded per-chunk event queue. Record large-file peak memory in benchmarks; no benchmark result exists yet.

## Commands and evidence policy

From the repository root, run `cargo fmt --check`, `cargo check`, `cargo test`, and `cargo clippy --all-targets --all-features -- -D warnings`. Execute Windows-only destination and promotion tests on Windows; cross-compilation alone does not validate their runtime behavior. Record exact commands and observed results in the completion report. Loopback server tests may require local-network permission in restricted environments.

The ignored local performance harness is opt-in and is not part of ordinary `cargo test`: run `cargo test --release local_performance_scenarios -- --ignored --nocapture`. It byte-verifies generated fixtures and reports wall time, aggregate throughput, peak server-observed active requests, queued-at-admission jobs, and output/temp-file counts. Record the environment and interpret paced-server results separately from unthrottled throughput; do not infer CPU or memory use from elapsed time.

## Later-phase tests

- SQLite migrations, interrupted-write recovery, WAL sidecar handling, DB corruption/migration fallback, and user-data deletion (only if the JSON snapshot model is ever replaced; the versioned JSON envelope, corruption sidecar, and repair behavior are covered today by the service harness).
- Packaged Windows validation: DPAPI roundtrip on a real Windows account, promotion/reservation runtime tests, installer smoke test as a standard user, and interactive GUI/accessibility review remain the release gate; the Linux sandbox cannot execute them.
- Cross-restart resume is covered by the engine adoption tests and the service harness (valid continuation, shrunk/missing/oversized partials, changed resource, 200-after-Range rejection, live-checkpoint publication); a full unforced process-restart drill in a packaged build remains part of the Windows release gate.
- Segmented download assembly correctness, range overlap/gap detection, and bounded per-host concurrency.
- Packaged Windows upgrade/uninstall and WebView2 online/offline distribution paths.

## Official references

Checked 2026-10-21:

- Tokio cancellation-safety notes: https://docs.rs/tokio/latest/tokio/macro.select.html
- Reqwest response body stream: https://docs.rs/reqwest/latest/reqwest/struct.Response.html
- Tauri Windows packaging: https://v2.tauri.app/distribute/windows-installer/
