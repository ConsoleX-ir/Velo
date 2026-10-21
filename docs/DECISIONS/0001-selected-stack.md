# ADR 0001: Selected Application Stack

- **Status:** Accepted
- **Date:** 2026-10-21

## Context

ConsoleX Velo needs a Windows-first desktop application with native filesystem and networking behavior, a responsive desktop UI, and a free/ad-free local product model.

## Decision

Use Tauri 2, Rust for the native core and download engine, Tokio for async orchestration, Reqwest for HTTP/HTTPS, React + TypeScript for the frontend, and SQLite for persistent metadata when persistence is introduced.

## Rationale

This is the selected project stack and balances a native Rust boundary with a web UI. The engine can be a Rust library and tested independently of Tauri. SQLite is reserved for durable metadata rather than made a prerequisite for the first transfer implementation.

## Consequences

- The UI/native IPC boundary requires explicit permissions, CSP, and DTO validation.
- Windows installer and WebView2 packaging choices require measurement and release decisions.
- Tokio filesystem methods are async interfaces but currently use blocking filesystem work behind the scenes.
- Exact crate versions/features are not selected until Phase 1 dependency review.

## Reconsideration trigger

Reopen only if a concrete, critical technical blocker is demonstrated for the selected stack or a required platform capability cannot be met.

## References

- Tauri security: https://v2.tauri.app/security/csp/
- Tauri Windows packaging: https://v2.tauri.app/distribute/windows-installer/
- Tokio filesystem: https://docs.rs/tokio/latest/tokio/fs/index.html
