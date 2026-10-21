# ConsoleX Velo Architecture

**Status:** Phase 0 baseline  
**Research checked:** 2026-10-21

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
            ├─ download engine (HTTP lifecycle, retries, streaming, cancellation)
            ├─ destination/file policy (path and collision rules)
            └─ repository interface (in-memory in Phase 1; SQLite in a later phase)
```

The frontend does not open arbitrary filesystem paths or perform network requests directly. The Tauri command layer exposes explicit operations and DTOs; it does not expose generic shell, filesystem, or HTTP capabilities. The engine is a Rust library boundary independent of Tauri so it can be tested without a webview. A repository abstraction is not required in Phase 1 unless needed to keep orchestration testable; do not add an abstraction without a concrete consumer.

## State ownership and communication

- The native core owns authoritative download lifecycle state and byte counters.
- UI state is a projection of native snapshots/events and may be reconstructed; it is not durable truth.
- Commands carry stable download IDs and validated request fields. Events are emitted only on meaningful changes or a configured progress interval/byte threshold.
- Event queues must be bounded or coalescing. A slow UI must not cause unbounded memory growth or block response-body consumption.
- Phase 1 can use an in-memory registry and per-download task handle/token. Do not persist transient progress as if it guaranteed crash recovery.

## Persistence model and later SQLite boundary

Phase 1 uses no database. Later metadata should distinguish durable user intent from volatile transport state. Candidate durable fields: ID, original URL (with credentials/query-secret handling policy), destination, chosen filename, creation time, last known state, expected size if known, bytes durably recorded, and structured terminal error. Persisted state must not claim an interrupted partial file is resumable until validators/range semantics and file verification are implemented.

SQLite is selected for local metadata. Database placement should be under the per-user application data directory, not beside the executable. Use schema migrations and transactions. WAL is an option to evaluate, not a default mandate: official SQLite documentation says WAL introduces `-wal`/`-shm` companion files and cannot operate over a network filesystem. Keep database and companions together, and test crash/reopen/migration behavior before enabling WAL.

## Error and shutdown model

Use typed error categories at the engine boundary: invalid request, destination conflict/permission/path error, request construction, DNS/connect/TLS/timeout, redirect policy/limit, non-success HTTP status, body stream error, length mismatch/incomplete response, write/flush/finalize error, retry exhausted, and cancelled. Preserve safe diagnostic context (status, category, attempt count); redact URL userinfo and sensitive query values from logs and UI.

On normal application shutdown, stop accepting new work, request cancellation of active Phase 1 jobs, await their cleanup for a bounded interval, and leave partial-file disposition explicit. Force termination can leave a partial file; without persistence/recovery this is reported as interrupted on next launch only after a later recovery phase exists.

## Security boundaries

- Strict Tauri capability allowlists, least-privilege command permissions, and a restrictive CSP configured in Tauri.
- Bundle local frontend assets; avoid remote scripts and runtime code loading.
- No generic shell invocation from the UI.
- Validate URL scheme and destination before work starts; use safe filename normalization and collision-resistant temporary-file creation.
- Avoid path traversal and symlink/reparse-point surprises; document Windows-specific checks and test them.
- Do not log credentials, authorization/cookie headers, full sensitive URLs, or downloaded content.
- Treat response headers, filenames, redirect targets, and remote error text as untrusted input.
- The download engine should not accept arbitrary caller-supplied sensitive headers in Phase 1. If later supported, redirect policy must strip credentials/cookies on origin changes and block HTTPS-to-HTTP downgrade by default.

## Packaging assumptions

Windows x64 is the initial packaging target. Tauri currently documents MSI (WiX) and NSIS setup executables; MSI creation is Windows-bound. WebView2 may be downloaded by bootstrapper or bundled. Select installer type and runtime strategy only after checking offline installation needs, signing, upgrades, and measured package size. Code signing certificate and publisher details are unresolved. The 100–130 MB installer goal is not a commitment.

## Verified facts, decisions, assumptions

- **Verified (official docs, checked 2026-10-21):** Tauri 2 supports a configured CSP and recommends avoiding remote scripts; Windows installers include MSI and NSIS options; Reqwest exposes streamed response bodies and configurable redirect policies; Tokio's filesystem API currently dispatches ordinary blocking file operations via its blocking pool; SQLite WAL uses companion files and has network-filesystem limitations.
- **Decisions:** selected stack above; isolate engine from Tauri; authoritative lifecycle state in native code; no Phase 1 database; user-selected paths; no downgrade redirects.
- **Assumptions:** Windows 10/11 x64 first, single-stream transfer, direct distribution, and per-user installation are provisional.
- **Unresolved:** exact crate versions/features, global concurrency limit, installer/runtime policy, support matrix, persistence schema, and resume protocol.

## Official references

URLs were checked on 2026-10-21; versions can change. Pin and review versions when implementation begins.

- Tauri CSP: https://v2.tauri.app/security/csp/
- Tauri capabilities: https://v2.tauri.app/security/capabilities/
- Tauri Windows installers: https://v2.tauri.app/distribute/windows-installer/
- Reqwest response streaming: https://docs.rs/reqwest/latest/reqwest/struct.Response.html
- Reqwest redirect handling: https://docs.rs/reqwest/latest/reqwest/redirect/
- Tokio filesystem behavior: https://docs.rs/tokio/latest/tokio/fs/index.html
- SQLite WAL: https://www.sqlite.org/wal.html
