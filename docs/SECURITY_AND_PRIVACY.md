# Security and Privacy Requirements

**Status:** Phase 0 baseline  
**Research checked:** 2026-10-21

## Privacy commitments

- Free and ad-free; no mandatory accounts, cloud sync, AI features, or telemetry.
- Download requests are initiated by the user. Phase 1 makes only the requested HTTP/HTTPS requests plus required redirects/retries.
- Store metadata locally. Define retention and deletion behavior before persistent history ships.
- Avoid collecting diagnostic data. If optional diagnostics are later proposed, make them opt-in, documented, minimal, and scrubbed.
- URLs can contain private tokens in query strings. Display/log redaction must remove userinfo and sensitive query values; database retention policy remains open.

## Threat boundaries

Assume URLs, redirect targets, headers, content-disposition names, response bodies, remote error messages, downloaded files, and local filenames are untrusted. Threats include malicious server content, path traversal, destination clobbering, redirect-based credential leakage, resource exhaustion, webview-to-native abuse, and accidental secret exposure in logs.

## Required controls

### Tauri and frontend

- Enable a strict, app-specific CSP. No remote script/CDN dependencies at runtime.
- Define least-privilege Tauri v2 capabilities and command permissions. Expose only narrow commands and typed data; never provide generic shell or filesystem access.
- Validate all IPC input on the Rust side; TypeScript validation is convenience, not a trust boundary.
- Avoid rendering remote strings as HTML; escape filenames and error text.
- Keep dependencies minimal and lock versions; review transitive dependency changes.

### Network

- Require `http` or `https`, reject URL userinfo, and reject HTTPS-to-HTTP redirects.
- Bound redirect hops, request/body duration, retries, diagnostic capture, and concurrent work.
- Do not pass authorization/cookie headers in Phase 1. If authentication is added later, define redirect origin rules and secret storage first.
- Validate TLS certificates/hostnames using the selected TLS backend defaults; do not provide an “accept invalid certificates” mode in initial release.
- Do not perform arbitrary URL preflight/DNS fetches that could create SSRF-like behavior; the user is explicitly requesting the URL, but local/private-network access policy still requires a product decision.
- Whether a manually entered user URL may target loopback/private-network addresses remains a product policy decision; resolve it before browser/link interception or background URL ingestion is added.

### Filesystem

- Require an explicit destination selected/provided by the user; no silent overwrite.
- Phase 1 does not support overwrite. Reject an existing final file and preserve it on any transfer failure or accepted cancellation. Re-check atomically during final promotion; fail without replacing if another process creates the path.
- Create a random sibling temp file with exclusive create. Promote only after validation through a same-volume no-replace operation; Rust `std::fs::rename` may replace the destination on supported platforms and is not sufficient by itself for this policy.
- Sanitize suggested names to a basename and handle Windows reserved names, separators, control characters, trailing dots/spaces, and collisions.
- Canonicalize the existing parent and reserve canonical case-insensitive destination keys in-process. Detect/reject reparse-point path components where possible; parent canonicalization alone does not eliminate a path race. Windows handle-based checks and junction/symlink tests are a release gate.
- Do not execute downloaded files or inspect their content beyond optional later checksum verification.

### Data and diagnostics

- Never log request headers, URL credentials, raw sensitive query strings, or file contents.
- Keep diagnostics bounded and sanitize CR/LF/control characters.
- Do not make download URLs or history available to the webview except when needed for the UI.
- Later SQLite database should use per-user app data ACLs, schema migrations, and safe backup/restore handling. Data-at-rest encryption and secure credential storage are unresolved because credentials are out of Phase 1.

## Security review gates

Before shipping: review Tauri command/capability surface, CSP, updater (if any), dependency audit output, redirect tests, path handling on Windows, installer signing, auto-update trust chain, and log redaction. A packaged build should be tested as a standard user with restrictive filesystem permissions. Do not claim a security audit from these requirements alone.

## Verified references

Checked 2026-10-21:

- Tauri CSP: https://v2.tauri.app/security/csp/
- Tauri capabilities: https://v2.tauri.app/security/capabilities/
- Reqwest redirect policy: https://docs.rs/reqwest/latest/reqwest/redirect/
- Rust file open options: https://doc.rust-lang.org/std/fs/struct.OpenOptions.html
