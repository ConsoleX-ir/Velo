# ADR 0002: Phase 1 Download Engine Scope and Lifecycle

- **Status:** Accepted for Phase 1 planning
- **Date:** 2026-10-21

## Context

The first implementation must establish correct basic HTTP/HTTPS downloads without implying mature pause, resume, recovery, or segmented transfer behavior.

## Decision

Phase 1 implements a single-stream GET into an exclusively created sibling temporary file, with explicit lifecycle states, bounded progress reporting, cancellation, bounded classified retries from byte zero, response-length validation where available, and promotion only after successful finalization. It uses an in-memory registry and no SQLite recovery. Phase 1 is no-clobber only: existing targets are rejected, and final promotion must be atomic and must fail if the target appeared after validation. Cancellation and finalization are serialized; once `Finalizing` begins, cancellation is rejected as too late. Each accepted job has one retained terminal outcome independent of progress delivery.

## Consequences

- A process crash can leave a partial temp file; startup recovery is deferred.
- Existing destination files are always preserved in Phase 1. There is no overwrite request option.
- No Pause, Interrupted, Verifying, Range, or resume state is exposed.
- Response size may be unknown; successful EOF is the completion signal when no trustworthy length is available.
- Only final HTTP 200 is accepted; the transfer rejects non-identity content encoding and validates Content-Length against bytes successfully written.
- Provisional defaults: 3 total attempts, exponential delays of 250 ms and 500 ms with up to +20% jitter, `Retry-After` capped at 30 s (the total retry-delay ceiling), 10 s connect timeout, 30 s read-inactivity timeout, and 24 h network-job deadline. They must be adjustable and tested, not treated as validated UX settings.
- Ordinary progress is at most 10 updates/second/job; state transitions are immediate and terminal outcomes are retained separately.
- Provisional admission cap is 4 active and 100 retained job records total, including unacknowledged terminal records. Excess start calls are rejected synchronously.
- Rust `std::fs::rename` can replace an existing destination. Windows no-clobber promotion must use a same-volume API without replace-existing or cross-volume-copy flags; Windows path/reparse-point behavior remains a release gate.

## References

- Reqwest response stream: https://docs.rs/reqwest/latest/reqwest/struct.Response.html
- Tokio cancellation safety: https://docs.rs/tokio/latest/tokio/macro.select.html
- Rust OpenOptions: https://doc.rust-lang.org/std/fs/struct.OpenOptions.html
- Rust rename semantics: https://doc.rust-lang.org/std/fs/fn.rename.html
- Windows MoveFileExW: https://learn.microsoft.com/windows/win32/api/winbase/nf-winbase-movefileexw
