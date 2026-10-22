# ConsoleX Velo Architecture

**Status:** Phase 1B managed Rust core; Tauri and frontend remain planned
**Research checked:** 2026-10-21

**Implementation update (2026-10-22):** The root-level Rust library includes the Phase 1A HTTP/HTTPS transfer core and Phase 1B `DownloadManager`, with FIFO admission, generation-bearing handles, cancellation, coalesced progress, retained terminal results, shutdown, and bounded strong-ETag-validated Range continuation after interrupted bodies. Phase 1D hardening rejects ambiguous Content-Length/Transfer-Encoding framing and prevents Range metadata from crossing redirects. The core remains independent of Tauri; the application service, frontend, and Tauri shell are not present. On Windows, the full test suite passed, including no-clobber promotion and case-insensitive destination reservation tests (65 passed, 0 failed).

## Chosen stack

- Desktop shell and native/webview bridge: Tauri 2.
- Native application and download core: Rust.
- Async orchestration: Tokio.
- HTTP client: Reqwest.
- UI: React + TypeScript.
- Persistent metadata: SQLite, introduced after the in-memory Phase 1 core contract is stable.

This is the current project decision. No Electron comparison is in scope absent a concrete blocker.

## Component boundaries

```text
React + TypeScript
  └─ narrow Tauri commands/events; presentation state only
       └─ application service (validation, command authorization, orchestration)
            ├─ managed download engine (HTTP lifecycle, streaming, cancellation)
            ├─ destination/file policy (path and collision rules)
            └─ in-memory job registry (Phase 1; no database)
```

The frontend does not open arbitrary filesystem paths or perform network requests directly. The Tauri command layer exposes explicit operations and DTOs; it does not expose generic shell, filesystem, or HTTP capabilities. The Phase 1 engine is a Rust library boundary independent of Tauri so it can be tested without a webview. No application-service, repository, workspace, or UI scaffold currently exists in the repository. A repository abstraction is not required in Phase 1 unless needed to keep orchestration testable; do not add one without a concrete consumer.

## State ownership and communication

- The native core owns authoritative download lifecycle state and byte counters.
- UI state is a projection of native snapshots/events and may be reconstructed; it is not durable truth.
- Commands carry stable download IDs and validated request fields. State transitions are emitted immediately; ordinary progress is limited to at most 10 updates per second per job.
- Progress is latest-value/coalesced, with at most one pending snapshot per job. Terminal outcome is immutable and separately retained in the job registry until acknowledged; it is not an ordinary event that a bounded queue can drop.
- Phase 1B uses an in-memory registry, one task per active job, and caller-ID plus generation handles. Interrupted response bodies can continue with up to three strict `If-Range` requests using a known total and strong ETag; this does not provide restart recovery. Byte-zero retries remain deferred.
- Provisional scheduler cap: 4 active jobs and 100 retained job records, including terminal records not yet acknowledged; reject excess synchronously. Keep the fixed cap until benchmarks justify changing it; adaptive concurrency is deferred.

## Persistence model and later SQLite boundary

Phase 1 uses no database. Later metadata should distinguish durable user intent from volatile transport state. Candidate durable fields: ID, original URL (with credentials/query-secret handling policy), destination, chosen filename, creation time, last known state, expected size if known, bytes durably recorded, and structured terminal error. Persisted state must not claim an interrupted partial file is resumable until validators/range semantics and file verification are implemented.

SQLite is selected for local metadata. Database placement should be under the per-user application data directory, not beside the executable. Use schema migrations and transactions. WAL is an option to evaluate, not a default mandate: official SQLite documentation says WAL introduces `-wal`/`-shm` companion files and cannot operate over a network filesystem. Keep database and companions together, and test crash/reopen/migration behavior before enabling WAL.

## Error and shutdown model

Use typed error categories at the engine boundary: invalid request, destination conflict/permission/path error, request construction, DNS/connect/TLS/timeout, redirect policy/limit, non-success HTTP status, body stream error, length mismatch/incomplete response, write/flush/finalize error, worker panic, cleanup failure, and cancellation. Preserve safe diagnostic context (status and category); redact URL userinfo and sensitive query values from logs and UI.

The manager's idempotent shutdown closes admission under the same lock used for dispatch, cancels queued and cancellable active jobs, allows finalizing jobs to finish, and waits until accepted jobs have terminal results and cleanup is complete. The application may impose its own exit deadline; abandoning that wait does not make cleanup complete. Force termination can leave a temporary file and cannot guarantee terminal publication; crash recovery is deferred.

## Security boundaries

- Strict Tauri capability allowlists, least-privilege command permissions, and a restrictive CSP configured in Tauri.
- Bundle local frontend assets; avoid remote scripts and runtime code loading.
- No generic shell invocation from the UI.
- Validate URL scheme and destination before work starts; use safe filename normalization and collision-resistant temporary-file creation.
- Avoid path traversal and symlink/reparse-point surprises; document Windows-specific checks and test them.
- Do not log credentials, authorization/cookie headers, full sensitive URLs, or downloaded content.
- Treat response headers, filenames, redirect targets, and remote error text as untrusted input.
- The download engine should not accept arbitrary caller-supplied sensitive headers in Phase 1. If later supported, redirect policy must strip credentials/cookies on origin changes and block HTTPS-to-HTTP downgrade by default.
- Phase 1 rejects existing final destinations and uses same-directory exclusive temp creation followed by an atomic same-volume no-replace promotion. Do not use `std::fs::rename` as the no-clobber operation on Windows: its documented behavior may replace an existing target. A direct, target-specific Windows API dependency may be justified for this narrow operation.
- The shared manager registry reserves canonical destinations across all manager clones. Windows reservations compare canonical paths using ordinal case-insensitive comparison. Atomic no-replace promotion handles cross-process/external races. Reparse points and path-component races remain a Windows release-review concern.

## Packaging assumptions

Windows x64 is the initial packaging target. Tauri currently documents MSI (WiX) and NSIS setup executables; MSI creation is Windows-bound. WebView2 may be downloaded by bootstrapper or bundled. Select installer type and runtime strategy only after checking offline installation needs, signing, upgrades, and measured package size. Code signing certificate and publisher details are unresolved. The 100–130 MB installer goal is not a commitment.

## Verified facts, decisions, assumptions

- **Verified (official docs, checked 2026-10-21):** Tauri 2 supports a configured CSP and recommends avoiding remote scripts; Windows installers include MSI and NSIS options; Reqwest exposes streamed response bodies and configurable redirect policies; Tokio's filesystem API currently dispatches ordinary blocking file operations via its blocking pool; SQLite WAL uses companion files and has network-filesystem limitations.
- **Decisions:** selected stack above; isolate engine from Tauri; authoritative lifecycle state in native code; no Phase 1 database; user-selected paths; no downgrade redirects; Phase 1 no-clobber output; cancellation is rejected once `Finalizing` begins; terminal results are retained separately from progress.
- **Assumptions:** Windows 10/11 x64 first, single-stream transfer, direct distribution, and per-user installation are provisional.
- **Unresolved:** exact crate versions/features, whether to tune the provisional concurrency cap, installer/runtime policy, support matrix, persistence schema, durable user-controlled resume policy, and segmented-transfer protocol.

## Official references

URLs were checked on 2026-10-21; versions can change. Pin and review versions when implementation begins.

- Tauri CSP: https://v2.tauri.app/security/csp/
- Tauri capabilities: https://v2.tauri.app/security/capabilities/
- Tauri Windows installers: https://v2.tauri.app/distribute/windows-installer/
- Reqwest response streaming: https://docs.rs/reqwest/latest/reqwest/struct.Response.html
- Reqwest redirect handling: https://docs.rs/reqwest/latest/reqwest/redirect/
- Tokio filesystem behavior: https://docs.rs/tokio/latest/tokio/fs/index.html
- SQLite WAL: https://www.sqlite.org/wal.html
- Rust `std::fs::rename` behavior: https://doc.rust-lang.org/std/fs/fn.rename.html
- Windows `MoveFileExW`: https://learn.microsoft.com/windows/win32/api/winbase/nf-winbase-movefileexw
