# Test Strategy

**Status:** Phase 0 plan; no tests exist or have been run  
**Research checked:** 2026-10-21

## Phase 1 test layers

1. **Unit tests:** URL validation, state transition legality, error classification, retry/backoff calculation with injected clock/jitter, filename sanitization, byte-counter overflow handling, progress coalescing.
2. **Engine integration tests:** use a loopback ephemeral-port HTTP server with scripted responses; no public internet.
3. **Filesystem integration tests:** temporary directories; exact bytes, collision behavior, failure cleanup, overwrite preservation, rename/finalization behavior.
4. **Windows-specific tests:** reserved/device names, Unicode and long paths where supported, ACL/permission failure, junction/reparse-point cases, concurrent destination reservation, installer smoke test when packaging exists.
5. **Frontend/IPC tests later:** command authorization, DTO serialization, event ordering/coalescing, UI projection, and webview capability restrictions. Not a Phase 1 requirement if no UI is being built.

## Required scenario matrix

| Area | Cases | Required assertions |
|---|---|---|
| Validation | bad URL, unsupported scheme, embedded credentials, invalid destination | no network request or final file created; typed error |
| Success | empty body, known body, unknown length, chunked body | exact output bytes; byte count equals bytes written; terminal state order |
| Status | 3xx, 4xx, 5xx, unexpected 206, 204 | correct policy; no redirect body saved; no false completion |
| Redirect | valid same-origin, cross-origin, loop, over limit, HTTPS downgrade | bounded hops; policy enforced; no sensitive header leakage |
| Incomplete | declared larger/smaller, connection reset mid-body | fail; do not promote; accurate partial-byte/cleanup result |
| Cancellation | queued, connect, headers, body, backoff, finalization boundary | deterministic terminal state; no race into success after accepted cancellation |
| Retry | transient failure then success, permanent failure, retry exhaustion, Retry-After | exact attempt count and delay policy; retries restart at byte zero |
| Destination | existing file, two jobs same path, invalid/sanitized name, disk write failure | no unintended clobber; deterministic error and cleanup |
| Progress | fast chunks, slow chunks, slow subscriber | throttled/coalesced events; bounded queue; terminal event retained |

## Determinism and test hygiene

- Bind only loopback and use ephemeral ports. Ensure server tasks and temp directories are shut down in cleanup, including on test failure.
- Inject clock, jitter source, retry policy, and progress clock rather than sleeping for real backoff durations.
- Assert state transitions as an allowed sequence, not timing-sensitive exact timestamps.
- Avoid depending on public endpoints, DNS, external certificates, or uncontrolled bandwidth.
- Use fixed byte fixtures and a standard digest implementation already present in the dependency graph; do not add a dependency solely for test convenience without review.
- Keep tests independent and parallel-safe with unique directories/ports.

## Commands and evidence policy

Once the Rust crate exists, expected commands are `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings` (feature policy permitting), `cargo test --workspace`, and Windows CI tests. These commands have **not** been run because the workspace has no manifest or code. Add only commands that match the final workspace layout; report actual toolchain and output in release evidence.

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
