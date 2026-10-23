# ADR 0006: Phase X — cross-restart resume, hardened JSON persistence, bounded auto-retry

- **Status:** Accepted
- **Date:** 2026-10-22

## Context

After Phase 2 the app had durable history but interrupted transfers always restarted from byte zero, the history snapshot was a bare unversioned JSON array whose corruption silently discarded history on the next save, and there was no automatic retry at all. The engine already contained all safe-resume machinery (validated Range continuation, `If-Range`, byte-zero fallback); what was missing was a way to persist and re-adopt a checkpoint across process restarts without ever appending to an unverified partial.

## Decision

- **Cross-restart resume without a new trust model.** For unfinished jobs the service persists the temporary-file path, a lower-bound verified byte counter, and the strong-ETag/total checkpoint. Adoption is allowed only after on-disk re-verification (same directory as the destination, regular file, length at or beyond the persisted counter, below the recorded total) and server revalidation (`If-Range` plus `206` with a matching `Content-Range` start). Adopting renames the partial into a fresh process-owned temp name so two workers can never share one partial. Every failure mode — changed resource, invalid checkpoint, missing/shrunk partial — falls back to the existing documented byte-zero restart. Live checkpoints are recorded before the first body byte is written.
- **Stay with JSON; do not introduce SQLite.** The manager caps retained jobs at 100 and needs only full-snapshot reads/writes, so hardening the existing store was preferred over a database (consistent with ADR 0003). The snapshot becomes a versioned envelope (`version: 2`, v1 arrays still load) written via synced temp-file replacement with restrictive Unix permissions (0600), durable rename on Windows (`MoveFileEx` `WRITE_THROUGH`), parent-directory fsync on Unix, truncated-array repair to the last complete record, and a timestamped `.corrupt-<time>` sidecar that preserves unreadable snapshots and is surfaced to the user as a one-shot notice instead of being silently discarded.
- **Bounded automatic retry for transient failures only.** Connect/timeout/inactivity/incomplete-response classes and HTTP 5xx retry at most twice with injectable 2s/8s backoff. User cancellation and permanent failures are never retried automatically; a dedicated `Retrying` state prevents duplicate manual plus scheduled attempts; cancelling during the backoff window aborts the timer and deletes the stored partial.
- **Tauri-free service core.** `src-tauri` is split into `service.rs` (all validation, recovery, retry, persistence logic) plus a thin Tauri command shell, so the layer is testable without GTK/WebKit via a harness crate on any host.

## Consequences

- Interrupted transfers continue from the verified offset when the server supports strong validators; otherwise the honest byte-zero fallback applies, and this limitation is stated in the UI and docs rather than hidden.
- History is never silently discarded; a corrupt snapshot costs a sidecar file plus a user-visible notice.
- Retry behavior is deterministic and bounded; a dead network cannot loop forever, and manual retry remains available after budget exhaustion.
- The tauri-free service layer can be fully tested on non-Windows hosts; DPAPI roundtrips, Windows promotion/reservation runtime tests, and packaged installer behavior remain Windows release gates.

## References

- ADR 0003 (deferring SQLite), ADR 0005 (Tauri application boundary)
- docs/ARCHITECTURE.md "Phase X update" section
