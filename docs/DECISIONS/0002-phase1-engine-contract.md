# ADR 0002: Phase 1 Download Engine Scope and Lifecycle

- **Status:** Accepted for Phase 1 planning
- **Date:** 2026-10-21

## Context

The first implementation must establish correct basic HTTP/HTTPS downloads without implying mature pause, resume, recovery, or segmented transfer behavior.

## Decision

Phase 1 implements a single-stream GET into an exclusively created sibling temporary file, with explicit lifecycle states, bounded progress reporting, cancellation, bounded classified retries from byte zero, response-length validation where available, and promotion only after successful finalization. It uses an in-memory registry and no SQLite recovery.

## Consequences

- A process crash can leave a partial temp file; startup recovery is deferred.
- Existing destination files are preserved unless a future explicit overwrite contract is implemented and tested.
- No Pause, Interrupted, Verifying, Range, or resume state is exposed.
- Response size may be unknown; successful EOF is the completion signal when no trustworthy length is available.
- Windows rename, path, and reparse-point behavior must be tested before release.

## References

- Reqwest response stream: https://docs.rs/reqwest/latest/reqwest/struct.Response.html
- Tokio cancellation safety: https://docs.rs/tokio/latest/tokio/macro.select.html
- Rust OpenOptions: https://doc.rust-lang.org/std/fs/struct.OpenOptions.html
