# ConsoleX Velo Architecture

**Status:** Managed Rust core and Windows Tauri + React application with local history
**Research checked:** 2026-10-21

**Implementation update (2026-10-22):** The root-level Rust library includes the HTTP/HTTPS transfer core and managed `DownloadManager`, with FIFO admission, generation-bearing handles, cancellation, coalesced progress, retained terminal results, shutdown, and bounded strong-ETag-validated Range continuation after interrupted bodies. It also supports chunk-boundary pause/resume. `src-tauri/` contains the native Tauri application service and the React/TypeScript frontend lives at the repository root. The service validates UI requests, owns the manager, maps snapshots into frontend DTOs, sends state updates as Tauri events, and persists local history. Packaging and interactive GUI behavior still require validation on the target Windows setup.

**UI refinement update (2026-10-22):** The Aurora palette is now restrained, light and dark surfaces share theme-aware contrast tokens, and navigation, toolbar, and dialog use selective CSS glass materials with static edge reflections and blur. Navigation movement uses a short reduced-motion-aware CSS transition; no animation or chart framework was added. Active download speed charts consume the native DTO's measured bytes-per-second value, sampling at most once per second into a 36-value per-generation session buffer. SVG sparklines are hidden outside `Downloading`; terminal states never show a live rate. The custom V/download mark is the bundled frontend mark and source for the Windows ICO. No engine DTO or lifecycle behavior changed.

**Application update (2026-10-22):** Tailwind CSS v4 is integrated through its Vite plugin, with existing CSS variables mapped to semantic utility tokens. The small `src/components/ui/button.tsx` component follows the shadcn/ui composition pattern without adding a runtime component dependency. Downloads can pause at a chunk boundary and resume in the same session only when the engine has a known total and strong ETag checkpoint. A JSON snapshot under the per-user app-data directory stores history and DPAPI-protected original URLs. On restart, unfinished jobs are marked interrupted and retry from byte zero; persisted partials are not advertised as restart-resumable. Completed history is checked against the saved file length.

## Chosen stack

- Desktop shell and native/webview bridge: Tauri 2.
- Native application and download core: Rust.
- Async orchestration: Tokio.
- HTTP client: Reqwest.
- UI: React + TypeScript.
- Persistent metadata: atomic JSON snapshot in per-user app data; SQLite remains a future option if scale or query needs require it.

This is the current project decision. No Electron comparison is in scope absent a concrete blocker.

## Component boundaries

```text
React + TypeScript
  └─ narrow Tauri commands/events; presentation state only
       └─ application service (validation, command authorization, orchestration)
            ├─ managed download engine (HTTP lifecycle, streaming, cancellation)
            ├─ destination/file policy (path and collision rules)
            ├─ in-memory job registry
            └─ JSON history snapshot with Windows DPAPI URL protection
```

The frontend does not open arbitrary filesystem paths or perform network requests directly. The Tauri command layer exposes explicit operations and DTOs; it does not expose generic shell, filesystem, or HTTP capabilities. The native service invokes the manager for admission and cancellation, listens to each accepted handle's coalesced latest-value progress, and emits versioned UI snapshots. Startup subscribes to the event before requesting the initial snapshot; DTO revisions prevent older snapshots from replacing newer events. The frontend never treats itself as the source of transfer truth. The Phase 1 engine remains a Rust library boundary independent of Tauri, tested without a webview.

The native app exposes list, start, cancel, pause, resume, retry, and acknowledge operations. Destination selection uses the Tauri dialog plugin's save dialog; the selected path is still validated in Rust. File opening and folder reveal are intentionally absent. Tauri capability permissions are limited to core defaults and save-dialog access. The application waits for manager shutdown before allowing the normal close event to complete.

The frontend uses CSS design tokens and Tailwind utilities, with a small local Button component. Dark and light palettes and reduced-motion preference are stored locally in webview localStorage. The categories view explains that categories are unavailable. Download history persists across launches.

## State ownership and communication

- The native core owns authoritative download lifecycle state and byte counters.
- UI state is a projection of native snapshots/events. A JSON snapshot preserves history, destination, counters, and DPAPI-encrypted original URL; active transport state is not durable truth.
- Commands carry stable download IDs and validated request fields. State transitions are emitted immediately; ordinary progress is limited to at most 10 updates per second per job.
- Progress is latest-value/coalesced, with at most one pending snapshot per job. Terminal outcome is immutable and separately retained in the job registry until acknowledged; it is not an ordinary event that a bounded queue can drop.
- The engine uses an in-memory registry, one task per active job, and caller-ID plus generation handles. Interrupted response bodies can continue with up to three strict `If-Range` requests using a known total and strong ETag. User pause/resume retains a validated same-session checkpoint. Restart recovery marks unfinished work Interrupted; Retry starts at byte zero.
- Provisional scheduler cap: 4 active jobs and 100 retained job records, including terminal records not yet acknowledged; reject excess synchronously. Keep the fixed cap until benchmarks justify changing it; adaptive concurrency is deferred.

## Persistence model and later SQLite boundary

The current app stores a JSON snapshot rather than a database because the manager caps retained jobs at 100 and needs only full-snapshot reads/writes. It stores the original URL only after Windows DPAPI protection, plus destination, DTO, and counters. Writes go to a synced temporary snapshot then replace the prior file. A restart never trusts old partial bytes: unfinished rows become Interrupted and retry from byte zero. Completed file presence and size are checked at startup. SQLite may be introduced if filtering/query volume or concurrent writers outgrow the snapshot model; migration must preserve these rules.

If SQLite is introduced later, keep it under per-user app data, use schema migrations and transactions, and evaluate WAL with its companion-file and network-filesystem constraints.

## Error and shutdown model

Use typed error categories at the engine boundary: invalid request, destination conflict/permission/path error, request construction, DNS/connect/TLS/timeout, redirect policy/limit, non-success HTTP status, body stream error, length mismatch/incomplete response, write/flush/finalize error, worker panic, cleanup failure, and cancellation. Preserve safe diagnostic context (status and category); redact URL userinfo and sensitive query values from logs and UI.

The manager's idempotent shutdown closes admission under the same lock used for dispatch, cancels queued and cancellable active jobs, allows finalizing jobs to finish, and waits until accepted jobs have terminal results and cleanup is complete. On a later launch, unfinished persisted rows are marked Interrupted and retry from byte zero. Force termination can leave a temporary file; the app does not trust that file for recovery.

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
- **Unresolved:** whether to tune the provisional concurrency cap, installer/runtime policy, support matrix, crash-safe resume across restarts, and segmented-transfer protocol.

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
