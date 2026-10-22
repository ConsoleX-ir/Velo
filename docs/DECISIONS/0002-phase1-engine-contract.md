# ADR 0002: Phase 1 Download Engine Scope and Lifecycle

- **Status:** Accepted for Phase 1 planning
- **Date:** 2026-10-21

## Context

The initial implementation established correct basic HTTP/HTTPS downloads without durable pause, restart recovery, or segmented transfer behavior. A later engine hardening step adds narrowly bounded continuation of an interrupted body within the same live job.

## Decision

Phase 1A provides the single-stream HTTP/HTTPS transfer primitive. Phase 1B manages jobs using that transfer path: caller-supplied IDs, generation-bearing handles, an in-memory FIFO queue, bounded active and retained capacities, latest-value progress, cooperative cancellation, panic supervision, terminal-result acknowledgement, and idempotent shutdown. Each job begins with one full GET. If its response is interrupted after streaming starts, the engine may make up to three `Range` continuation requests only when the full response supplied a known length and strong ETag. Every continuation must validate status, exact byte offset, total length, response URL, and unchanged strong ETag before bytes are appended. Byte-zero retries, durable resume checkpoints, and SQLite recovery are deferred.

Phase 1 remains no-clobber: existing targets are rejected and promotion fails if a target appears after validation. Cancellation and finalization are serialized; after `Finalizing` begins cancellation returns `TooLate`. A cancellation accepted earlier yields `Cancelled` only after cleanup succeeds; cleanup failure yields `Failed`. Each accepted job has one retained terminal result independent of progress delivery. Terminal publication, active-slot release, and reservation release share one idempotent registry commit path.

## Consequences

- A process crash can leave a partial temp file; startup recovery is deferred.
- Existing destination files are always preserved in Phase 1. There is no overwrite request option.
- No user-controlled Pause, durable Interrupted state, crash recovery, or segmented-download state is exposed. Validated in-job Range continuation stays in `Downloading` and never resets byte progress.
- Response size may be unknown; successful EOF is the completion signal when no trustworthy length is available.
- The initial full GET accepts HTTP 200; an in-job continuation accepts HTTP 206 only after strict range, total, URL, and strong-ETag validation. Non-identity content encoding is rejected and lengths are checked against bytes successfully written.
- Phase 1 jobs use one initial GET and at most three validator-checked Range continuations after an interrupted body. General retries from byte zero, retry classification, jitter, and `Retry-After` are deferred. Timeout defaults remain as documented in `DOWNLOAD_ENGINE_DESIGN.md`; they are implementation defaults, not benchmark-validated UX budgets.
- Ordinary progress is at most 10 updates/second/job; state transitions are immediate and terminal outcomes are retained separately.
- Provisional admission cap is 4 active and 100 retained job records total, including unacknowledged terminal records. Excess start calls are rejected synchronously.
- `DownloadHandle` combines the required caller ID with an internal generation. Every manager control, lookup, snapshot, and acknowledgement validates the active generation; acknowledgement permits ID reuse without allowing stale handles to target the new job.
- `shutdown` closes admission under the registry lock, cancels queued and pre-finalizing active jobs, allows finalizing jobs to finish, and waits for all accepted jobs to commit terminal results. Repeated shutdown calls share the same close state and completion signal. Abrupt process/runtime termination is outside cleanup guarantees.
- Rust `std::fs::rename` can replace an existing destination. Windows no-clobber promotion must use a same-volume API without replace-existing or cross-volume-copy flags; Windows path/reparse-point behavior remains a release gate.

## References

- Reqwest response stream: https://docs.rs/reqwest/latest/reqwest/struct.Response.html
- Tokio cancellation safety: https://docs.rs/tokio/latest/tokio/macro.select.html
- Rust OpenOptions: https://doc.rust-lang.org/std/fs/struct.OpenOptions.html
- Rust rename semantics: https://doc.rust-lang.org/std/fs/fn.rename.html
- Windows MoveFileExW: https://learn.microsoft.com/windows/win32/api/winbase/nf-winbase-movefileexw
