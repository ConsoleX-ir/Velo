# Download Engine Design

**Status:** Phase 1B implemented in the Rust core; in-memory only
**Research checked:** 2026-10-21

## Current implementation boundary

The repository contains a reusable Reqwest transfer engine and a shared `DownloadManager` (`src/lib.rs`). The manager implements synchronous atomic admission, FIFO scheduling, configurable active and retained-record limits (defaults 4 and 100), generation-bearing handles, cooperative cancellation, latest-value `watch` progress, terminal lookup/acknowledgement, panic recovery, and idempotent shutdown. Each accepted job makes exactly one attempt. The manager reuses the existing HTTP response validation, timeout, redirect, exclusive-temp, and no-clobber promotion paths. The Rust test suite includes local HTTP and filesystem tests; its actual validation status is recorded in the project test evidence and must not be inferred from this design text.

Terminal publication, active-slot release, and destination-reservation release are performed together exactly once by the registry commit path. A transfer task owns the open temporary file and cleanup guard; the manager records its path before streaming. On unwind the guard makes a best-effort attempt, then the supervisor attempts cleanup and retains any failure in the terminal error. Normal shutdown waits for accepted jobs to reach terminal outcomes. Abrupt process termination cannot guarantee cleanup or a terminal result; a forced runtime shutdown can interrupt best-effort cleanup. Windows-specific promotion and case-insensitive destination reservation are implemented with target-specific Win32 calls and require actual Windows test execution for release confidence.

## Scope and non-goals

One accepted Phase 1B job performs one HTTP/HTTPS GET attempt and writes one response to one local file using a bounded-memory streaming loop. Phase 1B does not implement retries, pause/resume, crash recovery, segmented transfers, or checksum verification. Any future retry design must use a fresh byte-zero attempt and receive its own contract and tests.

## Public contract (Rust-shaped, illustrative)

```rust
struct DownloadRequest {
    id: DownloadId,
    url: String,
    destination: PathBuf,       // complete desired output path
}

enum DownloadState {
    Queued, Preparing, Downloading,
    Cancelling, Finalizing, Completed, Failed, Cancelled,
}

struct ProgressSnapshot {
    id: DownloadId,
    state: DownloadState,
    bytes_written: u64,
    total_bytes: Option<u64>,
}

fn start(request: DownloadRequest) -> Result<DownloadHandle, StartError>;
fn cancel(handle: &DownloadHandle) -> Result<CancelDisposition, ControlError>;
fn terminal_result(handle: &DownloadHandle) -> Result<Option<Arc<TerminalResult>>, ControlError>;
fn acknowledge_terminal(handle: &DownloadHandle) -> Result<(), ControlError>;
async fn shutdown(&self) -> ShutdownReport;
```

`start` is synchronous and accepts work only after URL/path preflight and an atomic registry commit. It rejects a duplicate retained ID, destination conflict, shutdown, or full retained capacity; rejected requests have no terminal outcome. The handle includes the caller-supplied ID and an internal generation. Every control, snapshot, lookup, and acknowledgement operation validates both against the currently registered record, so an old handle cannot affect a newly admitted job that reuses an acknowledged ID. Existing targets are always rejected; overwrite is unsupported.

`cancel` records a cooperative request and returns without waiting for file cleanup. It returns `TooLate` once `Finalizing` has won arbitration. Repeated cancellation while already cancelling is idempotent. A cancellation accepted before finalization ends in `Cancelled` only after cleanup succeeds; cleanup failure ends `Failed` and retains the cleanup path/error. Acknowledgement succeeds only after terminal publication, then removes the retained record and frees capacity and ID.

For every accepted job, the registry stores one immutable terminal result independently of progress delivery. A Tokio `watch` channel stores only the latest snapshot; state changes publish immediately and ordinary byte updates are limited to 10 per second per job. A slow or closed receiver cannot block transfer or displace the terminal result. The destination reservation is held through terminal commit and then released in the same critical section as active-slot release and terminal publication.

## Admission and resource limits

The defaults are 4 active jobs and 100 retained records total, including queued, active, and terminal-but-unacknowledged jobs. `DownloadManagerConfig` can set nonzero limits. Excess starts fail synchronously with `CapacityReached`; jobs over the active cap remain `Queued`. A record is removed only by explicit acknowledgement. Each accepted job gets one attempt; retries are deferred. Per active job, the engine holds bounded response data and one latest progress snapshot; memory does not scale with total file size.

## State machine

```text
Queued → Preparing → Downloading → Finalizing → Completed
                    │         │       │
                    │         │       └→ Failed
                    │         ├→ Failed
                    │         └→ Cancelling → Cancelled (cleanup succeeded)
                    │                              └→ Failed (cleanup failed)
                    └→ Cancelling → Cancelled (cleanup succeeded)
                                         └→ Failed (cleanup failed)
```

Any nonterminal state before `Finalizing` may transition to `Cancelling`; operational states may transition to `Failed`. Cancellation and the transition to `Finalizing` are serialized by the manager registry lock. If cancellation wins first, the task stops at a safe boundary and cleans up. If `Finalizing` wins first, `cancel` returns `TooLate`. There is no retry transition and no `Completed` result before successful promotion. A single terminal commit releases the active slot/reservation and publishes the result exactly once.

- `Queued`: accepted into the in-memory scheduler; no destination bytes written.
- `Preparing`: create the exclusive private temp file and request; URL and destination shape were validated before job acceptance.
- `Downloading`: successful eligible response accepted; body streamed and bytes written.
- `Cancelling`: cancellation requested; body stream and pending work are stopped and temporary output cleanup is attempted.
- `Finalizing`: EOF and response-length checks succeeded; the temp file is `sync_all`'d, closed as needed, and promoted with an atomic same-volume **no-replace** operation. An unexpected destination collision fails finalization and preserves that destination.
- `Completed`: final path is committed and byte count refers to bytes written/promoted.
- `Failed`: request, response, filesystem, worker-panic, finalization, or cleanup failure; the terminal error includes the safe typed cause and any cleanup failure/path.
- `Cancelled`: cancellation was accepted before finalization and temp-file cleanup completed. Cleanup failure is `Failed`, never successful cancellation.

There is no `Paused`, `Interrupted`, or `Verifying` state in Phase 1. Abrupt process exit is outside the live state machine; startup recovery is deferred.

## Validation and safe destination handling

1. Parse URL; accept only `http` and `https`; reject missing host, embedded username/password, unsupported scheme, and malformed URL before network I/O.
2. Resolve destination to an absolute path, require its parent directory to exist, canonicalize the parent, and reject a directory target, invalid filename, or existing final target. Phase 1 does not create parent directories and does not support overwrite.
3. Never write directly to the visible final path. Reserve a random, application-owned sibling temporary file using exclusive create (`create_new`) to avoid clobbering another job or attacker-created file.
4. Do not trust `Content-Disposition` as a path. If a suggested filename is used, parse only its basename, remove separators/control characters, reject `.`/`..`, reserved Windows device names and trailing dot/space, normalize length, then apply collision policy. Caller-selected path remains authoritative.
5. Reserve the canonical parent + case-insensitive Windows filename in the in-process registry before accepting the job. Also rely on atomic no-replace promotion to catch races with other processes and destination creation after validation.
6. Do not use `std::fs::rename` as a no-clobber primitive: Rust documents that it may replace an existing destination. On Windows, use a same-volume move/rename API without the replace-existing or copy-across-volume flags, and fail closed if its no-replace semantics cannot be guaranteed. Test target-exists and race cases on NTFS and any other supported filesystem. Promotion must never silently overwrite.
7. Require the parent directory to exist and reject reparse-point components in the selected destination path where they can be detected. Parent canonicalization reduces alias conflicts but does not eliminate races; implement Windows handle-based validation or document the residual same-user path-race threat and gate release on review. Do not claim canonicalization alone is race safe.
8. Phase 1 overwrite semantics are explicitly **unsupported**: existing targets fail with `DestinationExists`; the engine offers no overwrite flag. Any later overwrite feature requires a separately reviewed contract and tests proving what happens to the previous file on cancellation, failure, and crash.

## HTTP request and response contract

- Reuse a configured Reqwest `Client`. Defaults in this implementation are a 10-second connect timeout, 30-second read/inactivity timeout (bounds response-header waits and each body read), and a 24-hour Reqwest request deadline. These are implementation defaults, not measured UX budgets. There are no retry/backoff waits in Phase 1B.
- Send GET, request identity encoding (`Accept-Encoding: identity`) and build without transparent compression features unless byte-accounting is explicitly adapted and tested.
- Redirects: cap at 10 hops; validate every target scheme and reject userinfo; reject HTTPS-to-HTTP downgrade; allow HTTP-to-HTTPS. Loop, limit, and policy violations are typed terminal failures. Phase 1 sends no credentials/cookies/custom sensitive headers. If such headers are added later, strip them on origin changes and test the behavior; Reqwest's built-in sensitive-header stripping does not replace project policy.
- Disable automatic `Referer` forwarding so redirect chains cannot leak a source URL's query string. Never log full URLs by default; redact query values from surfaced diagnostics.
- Accept final status `200 OK` only for a full GET. This includes a valid zero-byte body when `Content-Length: 0` or EOF confirms empty content. Reject final `204`, `206`, other 2xx, and all non-2xx statuses; redirects are processed by the redirect policy and their bodies are never saved. Phase 1 never sends `Range`.
- A redirect response body is never written as file data. Body is consumed only after status/headers pass validation.
- Require absent or `identity` `Content-Encoding` (every header value is checked); reject other encodings, and any `Transfer-Encoding` other than `chunked`, so on-disk byte accounting is directly comparable to `Content-Length`. Reject `Content-Range` because Phase 1 never issues a range request. Record `Content-Length` as expected total, not proof of completion. Count bytes only after successful writes with checked `u64` addition. On EOF, if length is present and differs from bytes written, fail and never promote; reject overrun as soon as detected. If length is absent, successful protocol EOF is the completion signal and `total_bytes` remains unknown. For a close-delimited response without a declared length, the client cannot distinguish an intentional end from a peer that closed early; this limitation must be surfaced in product copy and no cryptographic integrity claim is made.
- Server-provided names, MIME types, and error bodies are untrusted. On non-200 HTTP responses, do not read or retain the response body; report the numeric status and a local safe message only.

## Streaming, accounting, and progress

Consume Reqwest chunks/stream and write each chunk to Tokio `File` with a fixed upper bound on in-flight body data. Use a partial-write loop with `write`, incrementing the checked byte counter for each successful returned byte count; treat `Ok(0)` as `WriteZero`. Do not use `write_all` where accurate partial-byte accounting is required, and do not collect the response into a `Vec` or buffer proportional to file size. Tokio filesystem APIs currently use blocking filesystem operations behind the async interface; this is a performance characteristic, not a promise of kernel-native async disk I/O.

Progress counts bytes successfully written in the single attempt. `total_bytes` is optional and comes from the accepted response. The internal byte counter is updated after every successful partial write; the public `watch` snapshot is published at most 10 ordinary times per second/job. State transitions are immediate. `watch` stores only its latest value; terminal results are retained separately in the registry, so slow subscribers cannot block disk writes, accumulate events, or lose a terminal result.

`Finalizing` calls `sync_all` on the temporary file before promotion. This incurs per-download cost and is included in benchmarks. The contract guarantees validated bytes and atomic no-replace visibility after promotion, but does not claim survival of sudden power loss for the final directory entry.

## Cancellation and cleanup

For accurate partial byte counts, implement writes as a loop over `AsyncWriteExt::write`, incrementing the checked count after each successful returned byte count (`Ok(0)` is `WriteZero`). Never drop a `write_all` future mid-operation: it may have written a prefix without returning that byte count.

Cancellation is cooperative while queued, connecting, waiting for headers, or reading the body. It is not selected against a pending file write: the current chunk write loop completes and counts each partial write before cancellation is observed. Cancellation is serialized against transition to `Finalizing` under the registry lock. Always close handles before deleting temp files. Cancellation cleanup success yields `Cancelled`; cleanup failure yields `Failed` with the temp path and cleanup error. On worker unwind, `TempGuard` makes a best-effort cleanup attempt and the supervisor makes an explicit second attempt before committing `Failed`. Runtime/process termination can prevent cleanup or terminal publication; no crash recovery is promised.

## Retries (deferred)

Every accepted Phase 1B job makes exactly one request attempt. Retry classification, `Retry-After`, jitter, delay injection, and retry-specific states/progress are not implemented. They require a later reviewed contract and tests.

## Deterministic test server and contract tests

Use a local in-process HTTP server bound to loopback on an ephemeral port; no external network. Script fixed bytes, chunked/slow streams, declared-length mismatch/truncation, status codes, redirects, connection reset, and delayed headers. Use deterministic cancellation and finalization gates. Assert exact output bytes; no final file on invalid/incomplete responses; existing destination bytes unchanged on failures/cancellation; exactly one terminal result per accepted job; progress snapshots coalesce and ordinary updates stay at or below 10/second/job; a stalled subscriber cannot block transfer or lose the terminal result. Assert FIFO order, no more than the configured active limit, the 100-record default (including unacknowledged terminal jobs), and synchronous capacity errors. Test concurrent duplicate IDs/destinations, stale generations after ID reuse, worker panic/cleanup failure, repeated shutdown, and Windows case-variant destination reservation and no-replace promotion.

## Open implementation decisions

- TLS backend and system proxy policy.
- Exact minimum Windows version/filesystems and handle-based mitigation for reparse-point/path races.
- Whether user-entered URLs may target loopback/private-network addresses; decide before adding link interception or background URL ingestion.
- Whether a later product version should add an explicit overwrite flow; Phase 1 itself is no-clobber only.
- Whether the provisional scheduler limits (4 active, 100 retained job records) should change after representative measurements.
- Retry classification and policy for a later phase.
- Exact cleanup recovery UX when the OS prevents deletion of a temporary file; Phase 1 returns `Failed` with the temporary path and cleanup error.

## Official references

Checked 2026-10-21:

- Reqwest streamed body: https://docs.rs/reqwest/latest/reqwest/struct.Response.html
- Reqwest redirect policy: https://docs.rs/reqwest/latest/reqwest/redirect/
- Reqwest client reuse: https://docs.rs/reqwest/latest/reqwest/struct.Client.html
- Tokio select and cancellation safety: https://docs.rs/tokio/latest/tokio/macro.select.html
- Tokio file operations and sync: https://docs.rs/tokio/latest/tokio/fs/struct.File.html
- Rust exclusive file creation options: https://doc.rust-lang.org/std/fs/struct.OpenOptions.html
- Rust rename behavior (including Windows): https://doc.rust-lang.org/std/fs/fn.rename.html
- Windows MoveFileEx semantics: https://learn.microsoft.com/windows/win32/api/winbase/nf-winbase-movefileexw
- Reqwest read and total request timeout semantics: https://docs.rs/reqwest/latest/reqwest/struct.ClientBuilder.html
