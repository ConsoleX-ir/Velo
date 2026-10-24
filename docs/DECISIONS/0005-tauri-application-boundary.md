# ADR 0005: Tauri application boundary

- **Status:** Accepted
- **Date:** 2026-10-22

## Context

The tested Rust download engine had no desktop shell or user-facing workflow. The application needs native path selection and manager-backed state without moving transfer authority into JavaScript or coupling the core to Tauri.

## Decision

- Keep the root `consolex-download-engine` crate independent and add the Tauri application as a nested `src-tauri` crate that depends on it by path.
- Keep React/TypeScript at the repository root, bundled locally with Vite.
- Let the Tauri service own manager integration, URL/path validation, stable UI identifiers, DTO mapping, lifecycle event publication, cancellation, and terminal acknowledgement.
- Use explicit Tauri commands and a save-dialog capability. Do not expose generic filesystem or shell access.
- Keep UI download records in memory for the app session. Acknowledge terminal outcomes only on explicit dismissal; do not imply persistence or recovery.
- Use CSS tokens for the Charcoal & signal red themes and motion effects; avoid a UI framework and animation dependency.

## Consequences

The engine remains independently testable and UI updates are projections of native snapshots. The nested Rust crate has its own Cargo lockfile and validation commands. Session state disappears on process exit. File opening, pause/resume, persistent history, and retry remain outside this integration. The Tauri configuration and native commands still require packaged Windows and interactive WebView validation before release.
