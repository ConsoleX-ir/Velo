# Download Engine Design

**Status:** Phase 0 contract for Phase 1  
**Research checked:** 2026-10-21

## Scope and non-goals

One job performs one HTTP/HTTPS GET and writes one response to one local file using a bounded-memory streaming loop. Phase 1 does not promise pause/resume, crash recovery, segmented transfers, checksum verification, or retrying from a partial offset. A retry starts a fresh attempt and must safely reset its private temporary file.

## Public contract (Rust-shaped, illustrative)

```rust
struct DownloadRequest {
    id: DownloadId,
    url: Url,
    destination: PathBuf,       // complete desired output path
}

enum DownloadState {
    Queued, Preparing, Downloading, Retrying,
    Cancelling, Finalizing, Completed, Failed, Cancelled,
}

struct Progress {
    id: DownloadId,
    state: DownloadState,
    bytes_written: u64,
    total_bytes: Option<u64>,
    attempt: u8,
    // rate/ETA may be added only with documented smoothing and reset semantics
}

async fn start(request: DownloadRequest) -> Result<DownloadHandle, StartError>;
async fn cancel(id: DownloadId) -> Result<(), ControlError>;
async fn acknowledge_terminal(id: DownloadId) -> Result<(), ControlError>;
// Handle exposes a bounded/coalescing progress view and a separately retained terminal result.
```

`start` validates and reserves the ID/destination before accepting work. Any ID still present in the registry (including a terminal record not yet acknowledged) or any conflicting destination is rejected and is not an accepted job. Phase 1 has no overwrite option: any existing destination is a conflict, and callers requesting overwrite must receive an explicit unsupported-policy error. Terminal outcomes are exactly one of `Completed { path, bytes_written }`, `Cancelled`, or `Failed { kind, safe_message, http_status?, attempts, partial_path?, cleanup_error? }`. A cancellation request accepted before finalization ends in `Cancelled` only if cleanup succeeds; cleanup failure ends in `Failed` with the partial path and cleanup error. Rejected/late cancellation does not change the eventual terminal result.

`cancel` returns `UnknownId` for an ID not in the registry and `AlreadyTerminal` for a terminal job; only an accepted cancellation of a nonterminal job returns success. `acknowledge_terminal` succeeds only for a terminal job and removes that retained record, freeing its capacity and ID. Start validation/capacity errors are synchronous and do not create jobs.

For every accepted job, the registry stores one immutable terminal result independently of progress delivery. The result remains queryable until the caller acknowledges/removes the job; it cannot be displaced by progress or lost because a progress receiver is slow or closed. The destination reservation is held until the terminal result has been committed to the registry.

## Admission and resource limits

Provisional Phase 1 limits are 4 active network jobs and 100 retained job records total, including queued, active, and terminal-but-unacknowledged jobs. Additional start requests fail synchronously with `CapacityReached`; they are not accepted jobs and have no terminal result. Jobs beyond the active limit remain `Queued`. A terminal record is removed only after explicit acknowledgement. These limits bound task, temp-file, registry, and progress state; they are adjustable after benchmark evidence. There is no adaptive concurrency in Phase 1. Per active job, keep at most one response chunk being processed and one latest progress snapshot; memory must not scale with total file size.

## State machine

```text
Queued → Preparing → Downloading → Finalizing → Completed
                    │         │       │
                    │         │       └→ Failed
                    │         ├→ Retrying → Preparing (fresh attempt; bounded)
                    │         ├→ Failed
                    │         └→ Cancelling → Cancelled (cleanup succeeded)
                    │                              └→ Failed (cleanup failed)
                    └→ Cancelling → Cancelled (cleanup succeeded)
                                         └→ Failed (cleanup failed)
```

Any nonterminal state before `Finalizing` may transition to `Cancelling`; all operational states may transition to `Failed`. State transitions and cancel acceptance are serialized by one per-job gate. The operation that wins the gate determines the result: if cancellation wins before `Finalizing`, the worker stops at a safe boundary and cleans up; if `Finalizing` wins, `cancel` returns `TooLate` and the job completes or fails finalization. `Retrying` either starts a fresh `Preparing` attempt or ends `Failed` when the attempt budget/deadline is exhausted. There is no accepted cancellation after finalization begins and no `Completed` result before successful promotion.

- `Queued`: accepted into the in-memory scheduler; no destination bytes written.
- `Preparing`: create the exclusive private temp file and request; URL and destination shape were validated before job acceptance.
- `Downloading`: successful eligible response accepted; body streamed and bytes written.
- `Retrying`: classified transient failure occurred and another bounded attempt is scheduled. Report attempt and delay; no concurrent attempt for the same job.
- `Cancelling`: cancellation requested; body stream and pending work are stopped and temporary output cleanup is attempted.
- `Finalizing`: EOF and response-length checks succeeded; the temp file is `sync_all`'d, closed as needed, and promoted with an atomic same-volume **no-replace** operation. An unexpected destination collision fails finalization and preserves that destination.
- `Completed`: final path is committed and byte count refers to bytes written/promoted.
- `Failed`: non-retryable error, retry budget exhausted, finalization failure, or cleanup failure; error includes the safe typed cause and any cleanup failure/path. The engine attempts to remove its temp file on every failure; if removal fails, it reports the remaining path and cleanup error.
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

- Reuse a configured Reqwest `Client`. Provisional defaults, to be configurable and adjusted only with evidence: 10-second connect timeout, 30-second body read/inactivity timeout (reset after each successful read), and a 24-hour network-job deadline from the first request attempt through response EOF, including retries/backoff. The deadline does not interrupt cleanup or a `Finalizing` phase already begun. Tests inject a clock and use short values. These are planning defaults, not validated user-facing settings.
- Send GET, request identity encoding (`Accept-Encoding: identity`) and build without transparent compression features unless byte-accounting is explicitly adapted and tested.
- Redirects: cap at 10 hops; validate every target scheme and reject userinfo; reject HTTPS-to-HTTP downgrade; allow HTTP-to-HTTPS. Loop, limit, and policy violations are typed terminal failures. Phase 1 sends no credentials/cookies/custom sensitive headers. If such headers are added later, strip them on origin changes and test the behavior; Reqwest's built-in sensitive-header stripping does not replace project policy.
- Disable automatic `Referer` forwarding so redirect chains cannot leak a source URL's query string. Never log full URLs by default; redact query values from surfaced diagnostics.
- Accept final status `200 OK` only for a full GET. This includes a valid zero-byte body when `Content-Length: 0` or EOF confirms empty content. Reject final `204`, `206`, other 2xx, and all non-2xx statuses; redirects are processed by the redirect policy and their bodies are never saved. Phase 1 never sends `Range`.
- A redirect response body is never written as file data. Body is consumed only after status/headers pass validation.
- Require absent or `identity` `Content-Encoding`; reject other encodings so on-disk byte accounting is directly comparable to `Content-Length`. Reject `Content-Range` because Phase 1 never issues a range request. Record `Content-Length` as expected total, not proof of completion. Count bytes only after successful writes with checked `u64` addition. On EOF, if length is present and differs from bytes written, fail and never promote; reject overrun as soon as detected. If length is absent, successful protocol EOF is the completion signal and `total_bytes` remains unknown. For a close-delimited response without a declared length, the client cannot distinguish an intentional end from a peer that closed early; this limitation must be surfaced in product copy and no cryptographic integrity claim is made.
- Server-provided names, MIME types, and error bodies are untrusted. On non-200 HTTP responses, do not read or retain the response body; report the numeric status and a local safe message only.

## Streaming, accounting, and progress

Consume Reqwest chunks/stream and write each chunk to Tokio `File` with a fixed upper bound on in-flight body data. Use a partial-write loop with `write`, incrementing the checked byte counter for each successful returned byte count; treat `Ok(0)` as `WriteZero`. Do not use `write_all` where accurate partial-byte accounting is required, and do not collect the response into a `Vec` or buffer proportional to file size. Tokio filesystem APIs currently use blocking filesystem operations behind the async interface; this is a performance characteristic, not a promise of kernel-native async disk I/O.

Progress counts bytes successfully written in the current attempt; it resets to zero when a retry starts from byte zero. `total_bytes` is optional and comes from the accepted response. Emit state transitions immediately. Ordinary progress is capped at 10 events/second/job (minimum 100 ms between ordinary updates); byte thresholds may suppress updates but never increase that frequency. Keep only the latest pending snapshot per job; do not enqueue every network chunk. Terminal result delivery is separate and retained in the registry, so progress coalescing cannot drop it. A slow subscriber cannot block disk writes or grow memory without bound.

`Finalizing` calls `sync_all` on the temporary file before promotion. This incurs per-download cost and is included in benchmarks. The contract guarantees validated bytes and atomic no-replace visibility after promotion, but does not claim survival of sudden power loss for the final directory entry.

## Cancellation and cleanup

For accurate partial byte counts, implement writes as a loop over `AsyncWriteExt::write`, incrementing the checked count after each successful returned byte count (`Ok(0)` is `WriteZero`). Never drop a `write_all` future mid-operation: it may have written a prefix without returning that byte count.

Cancellation is cooperative and observable while queued, connecting, waiting for headers, reading the body, or in retry backoff. During a write, finish the current chunk write (or correctly account for partial writes) before stopping. Tokio documents `write_all` as not cancellation-safe when dropped mid-operation, so do not select-cancel it mid-write. Serialize cancellation against transition to `Finalizing`: cancellation accepted first gives cleanup ownership; `Finalizing` first makes cancellation return `TooLate`. Always close handles before deleting temp files. Cancellation cleanup success yields `Cancelled`; cleanup failure yields `Failed` with the partial path and cleanup error. A terminal result is stored separately from progress and remains queryable until acknowledged.

## Retry policy

Effective wait is `min(30 s, max(computed_backoff, parsed_retry_after))`; invalid or past `Retry-After` values are ignored. Parse both delta-seconds and HTTP-date forms.

Provisional policy: 3 total attempts including the first; retry delays are 250 ms then 500 ms, each with up to +20% injected jitter. For 429/503, parse a valid delta-seconds or HTTP-date `Retry-After` and wait for the greater of the computed delay and server delay, capped at 30 seconds. The 24-hour network-job deadline still applies. Retry connect timeouts, temporary name-resolution/connect errors, connection resets before headers, interrupted body reads, body-inactivity timeouts, and statuses 408, 429, 500, 502, 503, 504. Do not retry TLS certificate/hostname failures, malformed URLs, redirect policy errors, all other HTTP statuses, local filesystem errors, destination conflicts, cancellation, or clean-EOF length mismatch. If the HTTP/TLS stack cannot distinguish a certificate or permanent resolver error from a transient connect error, classify it as permanent. Exhaustion ends `Failed` with attempt count. Before every retry, truncate and seek the private temp file to byte zero; reset attempt progress and never append. Defaults remain provisional until tested and benchmarked.

## Deterministic test server and contract tests

Use a local in-process HTTP server bound to loopback on an ephemeral port; no external network. Provide scripted endpoints for fixed bytes, chunked/slow streaming, declared-length mismatch/truncation, status codes, redirect chain/loop/downgrade, connection reset, delayed headers, and retry-then-success. Inject clock, jitter, timeout values, and cancellation gates. Acceptance criteria: exact byte-for-byte output for 0-byte, 1 MiB, and 64 MiB fixtures; no final file on invalid/incomplete response; existing destination bytes unchanged on all failures/cancellation; exactly one terminal result per accepted job; retry count/delays match injected schedule and attempts restart at byte zero; progress never exceeds 10 ordinary updates/second/job while state changes are immediate; one latest progress snapshot per job at most; a stalled progress consumer does not block transfer or lose the terminal result. Assert no more than 4 active and 100 retained job records (including unacknowledged terminal records), and synchronous `CapacityReached` beyond the cap. Add filesystem cases for two in-process jobs targeting the same path (exactly one accepted), external target creation before promotion (fail without overwrite), cleanup failure, Unicode and reserved Windows names, long paths within the selected OS policy, rename collision on Windows, and reparse points/junctions on Windows CI.

## Open implementation decisions

- Reqwest/Tokio versions and minimal feature set; TLS backend and system proxy policy.
- Exact minimum Windows version/filesystems and handle-based mitigation for reparse-point/path races.
- Whether user-entered URLs may target loopback/private-network addresses; decide before adding link interception or background URL ingestion.
- Whether a later product version should add an explicit overwrite flow; Phase 1 itself is no-clobber only.
- Whether the provisional scheduler limits (4 active, 100 retained job records) should change after representative measurements.
- Whether the provisional retry and timeout defaults need adjustment based on test and benchmark evidence.
- Semantics of cleanup failure if the OS prevents deletion of an open/locked temp file; Phase 1 must return `Failed` with its path and preserve it for user action.
- Loopback server/test utilities dependency choice versus a small hand-written test server.

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
