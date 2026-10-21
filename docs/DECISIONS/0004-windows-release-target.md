# ADR 0004: Windows Packaging Goal and Measurement Policy

- **Status:** Proposed planning decision
- **Date:** 2026-10-21

## Context

The product is Windows-first and has an initial desired installer size of approximately 100–130 MB, but there is no build or package to measure.

## Decision

Treat 100–130 MB as a planning goal only. Defer installer format, WebView2 runtime mode, install scope, and signing decisions until a buildable app and deployment requirements exist. Measure clean release artifacts and installed footprint under the procedure in `docs/PERFORMANCE_PLAN.md`.

## Consequences

- No size guarantee is made.
- MSI and NSIS are candidates; WebView2 bundling affects package size and offline behavior.
- Signing identity and minimum Windows version remain open.

## References

- Tauri Windows installer: https://v2.tauri.app/distribute/windows-installer/
