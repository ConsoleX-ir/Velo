# Test Strategy

**Status:** Phase 0 plan; no tests exist or have been run  
**Research checked:** 2026-10-21

## Phase 1 test layers

1. **Unit tests:** URL validation, state transition legality, error classification, retry/backoff calculation with injected clock/jitter, filename sanitization, byte-counter overflow handling, progress coalescing.
2. **Engine integration tests:** use a loopback ephemeral-port HTTP server with scripted responses; no public internet. Compare output bytes directly so no hash dependency is needed for the fixture gate.
3. **Filesystem integration tests:** temporary directories; exact bytes, collision behavior, failure cleanup, existing-file preservation, no-replace promotion, and finalization behavior.
4. **Windows-specific tests:** reserved/device names, Unicode and long paths where supported, ACL/permission failure, junction/reparse-point cases, concurrent destination reservation, installer smoke test when packaging exists.
5. **Frontend/IPC tests later:** command authorization, DTO serialization, event ordering/coalescing, UI projection, and webview capability restrictions. Not a Phase 1 requirement if no UI is being built.

## Required scenario matrix

| Area | Cases | Required assertions |
|---|---|---|
| Validation | bad URL, unsupported scheme, embedded credentials, invalid destination | no network request or final file created; typed error |
| Success | empty body, known body, unknown length, chunked body | exact output bytes; byte count equals bytes written; terminal state order |
| Status | final 200, 204, 206, other 2xx, 3xx, 4xx, 5xx | only 200 accepted; no redirect body saved; no false completion |
| Redirect | valid same-origin, cross-origin, loop, over limit, HTTPS downgrade | bounded hops; policy enforced; no sensitive header leakage |
| Incomplete | declared larger/smaller, connection reset mid-body | fail; do not promote; accurate partial-byte/cleanup result |
| Cancellation | queued, connect, headers, body, backoff, race with finalization | accepted cancel yields Cancelled only after cleanup; cancel after Finalizing returns TooLate; exactly one terminal outcome |
| Retry | transient failure then success, permanent failure, retry exhaustion, Retry-After | exact attempt count and delay policy; retries restart at byte zero |
| Destination | existing file, two jobs same path, external target created before promotion, invalid/sanitized name, disk write failure, cleanup failure | no unintended clobber; exactly one same-process reservation winner; deterministic error and cleanup path |
| Progress | fast chunks, slow chunks, slow subscriber | state transitions immediate; ordinary updates ≤10/second/job; at most one pending snapshot/job; terminal result retained separately |

## Determinism and test hygiene

- Bind only loopback and use ephemeral ports. Ensure server tasks and temp directories are shut down in cleanup, including on test failure.
- Inject clock, jitter source, retry policy, and progress clock rather than sleeping for real backoff durations.
- Assert state transitions as an allowed sequence, not timing-sensitive exact timestamps.
- Avoid depending on public endpoints, DNS, external certificates, or uncontrolled bandwidth.
- Use fixed byte fixtures and exact byte comparisons; do not add a digest dependency solely for test convenience.
- Keep tests independent and parallel-safe with unique directories/ports.

## Measurable Phase 1 acceptance gates

- Compare output byte-for-byte with 0-byte, 1 MiB, and 64 MiB fixtures; all successful cases must have exact expected length when known.
- For a scripted sequence that ends in a terminal result, assert exactly one terminal result per accepted job. Rejected `start` requests are not jobs and receive a synchronous start error.
- At an injected progress clock, assert no more than 10 ordinary progress snapshots per second per job; state transitions do not wait for that interval. Hold the progress subscriber stalled and assert one pending latest snapshot at most and a still-queryable terminal result.
- For retry tests, use the provisional 3-attempt schedule with injected jitter and clock; assert attempts start at byte zero and attempt-local progress resets.
- For every failure/cancellation test, assert an existing final file is byte-identical and any undeleted temporary path is included in a `Failed` result.
- Reject all final statuses except 200; accept absent Content-Length only after successful EOF, and reject both short and long bodies when Content-Length is present.
- Assert non-200 response bodies are neither saved nor retained in diagnostics; a final response with `Content-Range` or non-identity content encoding is rejected.
- For close-delimited responses without Content-Length, assert the protocol EOF outcome and unknown total; do not claim the client can detect a peer that closed early at a syntactically valid boundary.
- Verify in code review and stress test that response bodies are streamed and there is no file-size-proportional buffer or unbounded per-chunk event queue. Record large-file peak memory in benchmarks; no benchmark result exists yet.

## Commands and evidence policy

Once the Rust crate exists, expected commands are `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test --workspace`, plus Windows CI filesystem tests. These commands have **not** been run because the workspace has no manifest or code. Add only commands that match the final workspace layout; report actual toolchain and output in release evidence.

## Later-phase tests

- SQLite migrations, interrupted-write recovery, WAL sidecar handling, DB corruption/migration fallback, and user-data deletion.
- Resume contract against servers that support/ignore/reject Range and change ETag/Last-Modified; partial-file validator consistency.
- Segmented download assembly correctness, range overlap/gap detection, and bounded per-host concurrency.
- Packaged Windows upgrade/uninstall and WebView2 online/offline distribution paths.

## Official references

Checked 2026-10-21:

- Tokio cancellation-safety notes: https://docs.rs/tokio/latest/tokio/macro.select.html
- Reqwest response body stream: https://docs.rs/reqwest/latest/reqwest/struct.Response.html
- Tauri Windows packaging: https://v2.tauri.app/distribute/windows-installer/
