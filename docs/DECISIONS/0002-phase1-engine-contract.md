# ADR 0002: Phase 1 Download Engine Scope and Lifecycle

- **Status:** Accepted for Phase 1 planning
- **Date:** 2026-10-21

## Context

The first implementation must establish correct basic HTTP/HTTPS downloads without implying mature pause, resume, recovery, or segmented transfer behavior.

## Decision

Phase 1A provides the single-stream HTTP/HTTPS transfer primitive. Phase 1B manages one-attempt jobs using that transfer path: caller-supplied IDs, generation-bearing handles, an in-memory FIFO queue, bounded active and retained capacities, latest-value progress, cooperative cancellation, panic supervision, terminal-result acknowledgement, and idempotent shutdown. Retries are deferred; each accepted job makes exactly one attempt. No SQLite recovery is included.

Phase 1 remains no-clobber: existing targets are rejected and promotion fails if a target appears after validation. Cancellation and finalization are serialized; after `Finalizing` begins cancellation returns `TooLate`. A cancellation accepted earlier yields `Cancelled` only after cleanup succeeds; cleanup failure yields `Failed`. Each accepted job has one retained terminal result independent of progress delivery. Terminal publication, active-slot release, and reservation release share one idempotent registry commit path.

## Consequences

- A process crash can leave a partial temp file; startup recovery is deferred.
- Existing destination files are always preserved in Phase 1. There is no overwrite request option.
- No Pause, Interrupted, Verifying, Range, or resume state is exposed.
- Response size may be unknown; successful EOF is the completion signal when no trustworthy length is available.
- Only final HTTP 200 is accepted; the transfer rejects non-identity content encoding and validates Content-Length against bytes successfully written.
- Phase 1B makes one attempt per accepted job; retries, jitter, and `Retry-After` are deferred. The transfer keeps the 10 s connect, 30 s inactivity, and 24 h request-timeout defaults documented in `DOWNLOAD_ENGINE_DESIGN.md`; these are implementation defaults, not benchmark-validated UX budgets.
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
