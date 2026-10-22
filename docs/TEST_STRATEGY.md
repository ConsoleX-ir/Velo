# Test Strategy

**Status:** Phase 1A/1B suite with Phase 1D adversarial HTTP coverage; Windows validation passed 65 tests on 2026-10-22
**Research checked:** 2026-10-22

## Phase 1 test layers

1. **Unit tests:** URL validation, response-header validation, partial-write accounting, cleanup error composition, progress coalescing, generation checks, and manager lifecycle behavior. Retry/backoff tests are deferred until retry implementation.
2. **Engine integration tests:** use a loopback ephemeral-port HTTP server with scripted responses; no public internet. Compare output bytes directly so no hash dependency is needed for the fixture gate.
3. **Filesystem integration tests:** temporary directories; exact bytes, collision behavior, failure cleanup, existing-file preservation, no-replace promotion, and finalization behavior.
4. **Windows-specific tests:** reserved/device names, Unicode and long paths where supported, ACL/permission failure, junction/reparse-point cases, concurrent destination reservation, installer smoke test when packaging exists.
5. **Frontend/IPC tests:** model tests cover revision-based merging, filtering, formatting, and URL/name validation; app tests cover persisted preferences and stale initial snapshots arriving after newer events. Tauri service tests cover command validation, safe errors, and a real loopback download. Capability restrictions and packaged WebView behavior remain manual release checks.

## Required scenario matrix

| Area | Cases | Required assertions |
|---|---|---|
| Validation | bad URL, unsupported scheme, embedded credentials, invalid destination | no network request or final file created; typed error |
| Success | empty body, known body, unknown length, chunked body | exact output bytes; byte count equals bytes written; terminal state order |
| Status | final 200, 204, 206, other 2xx, 3xx, 4xx, 5xx | only 200 accepted; no redirect body saved; no false completion |
| Redirect | valid same-origin, cross-origin, loop, over limit, HTTPS downgrade | bounded hops; policy enforced; no sensitive header leakage |
| Incomplete | declared larger/smaller, connection reset mid-body | fail; do not promote; accurate partial-byte/cleanup result |
| Cancellation | queued, connect, headers, body, backoff, race with finalization | accepted cancel yields Cancelled only after cleanup; cancel after Finalizing returns TooLate; exactly one terminal outcome |
| Managed jobs | duplicate IDs, destination conflicts, retained capacity, FIFO order, configured concurrency, acknowledgement, stale handles | admission is atomic; retained capacity includes queued and terminal-unacknowledged records; stale generations cannot act on reused IDs |
| Shutdown and worker failure | repeated/concurrent shutdown, queued cancellation, active cancellation, finalizing job, worker panic | admission closes once; no new dispatch after close; panic becomes one retained terminal failure; cleanup failure is preserved |
| Destination | existing file, two jobs same path, external target created before promotion, invalid/sanitized name, disk write failure, cleanup failure | no unintended clobber; exactly one same-process reservation winner; deterministic error and cleanup path |
| Progress | fast chunks, slow chunks, slow subscriber | state transitions immediate; ordinary updates ≤10/second/job; at most one pending snapshot/job; terminal result retained separately |

## Determinism and test hygiene

- Bind only loopback and use ephemeral ports. Ensure server tasks and temp directories are shut down in cleanup, including on test failure.
- Use deterministic server gates and state inspection instead of timing-sensitive sleeps where practical. Retry-clock/jitter injection is deferred with retry implementation.
- Assert state transitions as an allowed sequence, not timing-sensitive exact timestamps.
- Avoid depending on public endpoints, DNS, external certificates, or uncontrolled bandwidth.
- Use fixed byte fixtures and exact byte comparisons; do not add a digest dependency solely for test convenience.
- Keep tests independent and parallel-safe with unique directories/ports.

## Measurable Phase 1 acceptance gates

- Compare output byte-for-byte with 0-byte, 1 MiB, and 64 MiB fixtures; all successful cases must have exact expected length when known.
- For a scripted sequence that ends in a terminal result, assert exactly one terminal result per accepted job. Rejected `start` requests are not jobs and receive a synchronous start error.
- Assert progress snapshots coalesce to the latest value, ordinary updates are at most 10 per second/job, and state transitions publish immediately. A slow subscriber cannot block transfer or lose a separately queryable terminal result.
- Each job starts with one full GET and may make at most three in-job Range continuations after an interrupted body. Validate exact offset, unchanged strong ETag and total, final URL, redirect rejection without contacting the target, segment length/overrun, ambiguous framing rejection, bounded continuation count, and no promotion after a rejected range. Byte-zero retry behavior remains deferred.
- For every failure/cancellation test, assert an existing final file is byte-identical and any undeleted temporary path is included in a `Failed` result.
- Reject all final statuses except 200; accept absent Content-Length only after successful EOF, and reject both short and long bodies when Content-Length is present.
- Assert non-200 response bodies are neither saved nor retained in diagnostics; a full response with `Content-Range` or non-identity content encoding is rejected.
- For close-delimited responses without Content-Length, assert the protocol EOF outcome and unknown total; do not claim the client can detect a peer that closed early at a syntactically valid boundary.
- Verify in code review and stress test that response bodies are streamed and there is no file-size-proportional buffer or unbounded per-chunk event queue. Record large-file peak memory in benchmarks; no benchmark result exists yet.

## Commands and evidence policy

From the repository root, run `cargo fmt --check`, `cargo check`, `cargo test`, and `cargo clippy --all-targets --all-features -- -D warnings`. Execute Windows-only destination and promotion tests on Windows; cross-compilation alone does not validate their runtime behavior. Record exact commands and observed results in the completion report. Loopback server tests may require local-network permission in restricted environments.

The ignored local performance harness is opt-in and is not part of ordinary `cargo test`: run `cargo test --release local_performance_scenarios -- --ignored --nocapture`. It byte-verifies generated fixtures and reports wall time, aggregate throughput, peak server-observed active requests, queued-at-admission jobs, and output/temp-file counts. Record the environment and interpret paced-server results separately from unthrottled throughput; do not infer CPU or memory use from elapsed time.

## Later-phase tests

- SQLite migrations, interrupted-write recovery, WAL sidecar handling, DB corruption/migration fallback, and user-data deletion.
- Durable resume contract against servers that support/ignore/reject Range and change ETag/Last-Modified; checkpoint and partial-file consistency across process restart.
- Segmented download assembly correctness, range overlap/gap detection, and bounded per-host concurrency.
- Packaged Windows upgrade/uninstall and WebView2 online/offline distribution paths.

## Official references

Checked 2026-10-21:

- Tokio cancellation-safety notes: https://docs.rs/tokio/latest/tokio/macro.select.html
- Reqwest response body stream: https://docs.rs/reqwest/latest/reqwest/struct.Response.html
- Tauri Windows packaging: https://v2.tauri.app/distribute/windows-installer/
