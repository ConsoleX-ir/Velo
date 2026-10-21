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
    overwrite: bool,            // default false
    max_attempts: u8,           // bounded, default selected in config
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
// Handle exposes a bounded/coalescing progress receiver and a terminal result.
```

`start` validates and reserves the ID/destination before accepting work. A duplicate active ID or conflicting destination is rejected. Terminal results distinguish `Completed { path, bytes_written }`, `Cancelled { partial_path? }`, and `Failed { kind, safe_message, http_status?, attempts }`. The precise public DTO names may change during implementation while preserving these semantics.

## State machine

```text
Queued → Preparing → Downloading → Finalizing → Completed
                         │  │
                         │  ├→ Retrying → Preparing (fresh attempt; bounded)
                         │  ├→ Failed
                         │  └→ Cancelling → Cancelled
                         └→ Failed
Queued/Preparing → Cancelling → Cancelled
```

- `Queued`: accepted into the in-memory scheduler; no destination bytes written.
- `Preparing`: URL, destination, and collision policy validated; private temp file reserved; HTTP request starts.
- `Downloading`: successful eligible response accepted; body streamed and bytes written.
- `Retrying`: classified transient failure occurred and another bounded attempt is scheduled. Report attempt and delay; no concurrent attempt for the same job.
- `Cancelling`: cancellation requested; body stream and pending work are stopped and temporary output cleanup is attempted.
- `Finalizing`: EOF and response-length checks succeeded; flush/sync policy is applied; temp file is atomically promoted to final path subject to no-clobber/overwrite policy.
- `Completed`: final path is committed and byte count refers to bytes written/promoted.
- `Failed`: non-retryable error or retry budget exhausted; error includes safe typed cause. Partial/temp file cleanup policy is deterministic and reported.
- `Cancelled`: cancellation cleanup completed. If cleanup fails, return a cleanup detail and path so the user can recover/remove it; do not claim clean cancellation.

There is no `Paused`, `Interrupted`, or `Verifying` state in Phase 1. Abrupt process exit is outside the live state machine; startup recovery is deferred.

## Validation and safe destination handling

1. Parse URL; accept only `http` and `https`; reject missing host, embedded username/password, unsupported scheme, and malformed URL before network I/O.
2. Resolve destination to an absolute path and verify parent exists or can be created under an explicit policy. Reject directories, invalid filename, and existing target when overwrite is false.
3. Never write directly to the visible final path. Reserve a random, application-owned sibling temporary file using exclusive create (`create_new`) to avoid clobbering another job or attacker-created file.
4. Do not trust `Content-Disposition` as a path. If a suggested filename is used, parse only its basename, remove separators/control characters, reject `.`/`..`, reserved Windows device names and trailing dot/space, normalize length, then apply collision policy. Caller-selected path remains authoritative.
5. Re-check final path/collision at promotion time. Use same-volume rename semantics; Windows replacement behavior must be tested. If explicit overwrite is supported, replace only during finalization and document replacement guarantees. A failed download must not destroy a pre-existing file.
6. Treat reparse points/symlinks and parent-directory races as a Windows-specific hardening item. Define use of canonicalization/handle-based checks before implementation; do not imply canonicalization alone removes TOCTOU risk.

## HTTP request and response contract

- Reuse a configured Reqwest `Client`; set finite connect and whole-request/body timeouts (policy values are configuration decisions). No infinite hangs by default.
- Send GET, request identity encoding (`Accept-Encoding: identity`) and build without transparent compression features unless byte-accounting is explicitly adapted and tested.
- Redirects: cap at 10 hops; validate every target scheme and host; reject HTTPS-to-HTTP downgrade. Do not forward credentials or caller-sensitive headers across origins. Phase 1 has no arbitrary custom headers, cookies, or URL credentials. Redirect loop/limit/policy violations are terminal typed errors. Decide whether HTTP→HTTPS is allowed (expected yes).
- Accept successful response statuses appropriate to a full GET (normally 200; define 204/206 explicitly). Phase 1 does not issue Range. A 206 response to a non-range request is rejected unless protocol semantics and tests explicitly support it. Non-success statuses fail before opening/promoting output; retain status and a bounded, sanitized diagnostic only if safe.
- A redirect response body is never written as file data. Body is consumed only after status/headers pass validation.
- Record `Content-Length` when present as expected total, not as proof of completion. Count bytes after successful writes with checked `u64` addition. On EOF, if a trustworthy unencoded Content-Length exists and differs from bytes written, fail as incomplete/mismatched; never promote. If absent, successful EOF is the only available completion signal. Reject body length over declared length as soon as detected.
- Server-provided names, MIME types, and error bodies are untrusted. Do not allocate/read arbitrary error bodies; cap any diagnostic capture (or omit it).

## Streaming, accounting, and progress

Consume Reqwest chunks/stream and write each chunk to Tokio `File` with a fixed upper bound on in-flight body data. Do not collect the response into a `Vec` or buffer proportional to file size. Tokio filesystem APIs currently use blocking filesystem operations behind the async interface; this is a performance characteristic, not a promise of kernel-native async disk I/O.

Progress counts bytes successfully written. `total_bytes` is optional. Emit immediately on state changes and otherwise at most every 100 ms or after a configurable byte delta, whichever occurs first. Keep latest progress per job (watch/coalesced semantics); do not enqueue every network chunk. Terminal event must not be dropped. A slow UI cannot block disk writes or grow memory without bound.

`Finalizing` flushes and follows a documented durability policy. `sync_all` improves data/metadata durability at extra cost; whether Phase 1 uses it on every file is an open decision to settle before coding and benchmark. Promotion occurs only after length validation and successful finalization.

## Cancellation and cleanup

Cancellation is cooperative and observable at request, stream-next, backoff timer, write, and finalization boundaries. Use a cancellation token/channel and select against cancellable waits. Tokio documents `write_all` as not cancellation-safe when dropped mid-operation, so implementation must not casually cancel a partially progressed `write_all`: complete the current write or use a loop over cancellation-safe writes with correct partial-write accounting, then stop. Do not race cancel with promotion: transition to `Finalizing` establishes a defined point after which cancellation is rejected or reported as too late. Always close handles before deleting temp files. Cleanup errors are surfaced, not swallowed.

## Retry policy

Retries are bounded by `max_attempts` and a maximum delay. Use exponential backoff with jitter and a global cap; make timing injectable for deterministic tests. Retry only selected transient connect/DNS/timeouts, interrupted body reads, and selected status codes (e.g. 408, 429, 500, 502, 503, 504). Honor a valid bounded `Retry-After` for 429/503. Do not retry TLS certificate/hostname failures, invalid URL, redirect policy errors, 4xx other than explicit transient cases, disk errors, destination conflict, cancellation, or length mismatch by default. Exhaustion becomes `Failed` with attempt count. Every retry starts from byte zero in a newly truncated/recreated private temp file; no partial append/resume. Whether a timeout during a slow but progressing body is retriable is governed by explicit idle/overall timeout policy.

## Deterministic test server and contract tests

Use a local in-process HTTP server bound to loopback on an ephemeral port; no external network. Provide scripted endpoints for fixed bytes, chunked/slow streaming, declared-length mismatch/truncation, status codes, redirect chain/loop/downgrade, connection reset, delayed headers, and retry-then-success. Inject retry clock/jitter and cancellation timing where necessary. Assert exact file bytes/hash, state ordering, byte totals, terminal error kind, attempt count, destination preservation, temp cleanup, and upper bounds on progress event rate. Add filesystem cases for existing files, invalid names, permissions where portable, long/reserved Windows names, and reparse points on Windows CI.

## Open implementation decisions

- Reqwest/Tokio versions and minimal feature set; TLS backend and system proxy policy.
- Exact connect, inactivity, and overall timeouts; retry attempt and delay defaults.
- Whether parent directories are created automatically.
- Overwrite semantics and atomic replacement implementation on Windows.
- Disk flush/sync durability policy and semantics of cleanup failure.
- Loopback server/test utilities dependency choice versus a small hand-written test server.

## Official references

Checked 2026-10-21:

- Reqwest streamed body: https://docs.rs/reqwest/latest/reqwest/struct.Response.html
- Reqwest redirect policy: https://docs.rs/reqwest/latest/reqwest/redirect/
- Reqwest client reuse: https://docs.rs/reqwest/latest/reqwest/struct.Client.html
- Tokio select and cancellation safety: https://docs.rs/tokio/latest/tokio/macro.select.html
- Tokio file operations and sync: https://docs.rs/tokio/latest/tokio/fs/struct.File.html
- Rust exclusive file creation options: https://doc.rust-lang.org/std/fs/struct.OpenOptions.html
