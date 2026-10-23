# ADR 0007 — Desktop shell integration: tray lifecycle, custom title bar, and persistent default download folder

**Status:** Accepted (implemented in 0.4.0)
**Date:** 2026-10-23

## Context

Phase 2 shipped Velo as a decorated Tauri window whose close button ended the process. Three gaps remained against the release target:

1. Closing the window terminated the app, killing active downloads — unlike every established Windows download manager.
2. The native title bar visually clashed with the redesigned interface.
3. Users re-picked a full destination path for every download; there was no persistent default folder.

## Decisions

### Window lifecycle: hide-to-tray with an explicit Exit

- The main window runs with `decorations: false`. The webview draws a title bar whose Minimize/Maximize/Close buttons call the real window API (`core:window:allow-minimize`, `allow-toggle-maximize`, `allow-close`); dragging uses the `data-tauri-drag-region` hook (`allow-start-dragging`). No simulated controls.
- `WindowEvent::CloseRequested` is prevented and the window is hidden instead of destroyed. Active downloads live on the tokio runtime inside the managed `AppState`, so hiding the webview cannot cancel them.
- The tray (tauri `tray-icon` feature, using the bundled window icon) offers Show Velo, Hide Velo, and Exit Velo. Left click restores and focuses. Exit calls `AppHandle::exit(0)`, which flows through the existing `RunEvent::ExitRequested` handler: the manager is shut down, a final forced history persist runs, and only then does the process exit.
- `tauri-plugin-single-instance` is registered first; a second launch focuses the existing window. Duplicate engines, trays, and history writers are therefore impossible.

### Settings: one store, Rust-validated

- A new tauri-free `settings` module (mirroring `service`/`persistence`) owns `settings.json` — a versioned envelope written through the same synced temp-file + durable-rename helper as the download history. Version mismatch or corruption falls back to defaults rather than failing startup.
- The stored value is only the default download **directory**. It is validated (absolute, exists, is a directory, passes a create-and-delete write probe) before every save and on every read. An invalid saved directory is reported to the UI (`storedDirValid: false`) and falls back to the OS Downloads folder (resolved once via `app.path().download_dir()`), never silently to nothing.
- Downloads are started as `start_download(url, directory, file_name)`. The **native side** composes the final path with `join_destination`, which rejects relative directories, separators, traversal components, Windows-forbidden characters, trailing dot/space, and reserved device names (CON, COM1, …) before the engine's own absolute-path + no-overwrite validation runs. The webview never assembles paths itself.
- Per-download overrides change only that job's destination; the default stays untouched, and active downloads are never relocated.

### Capabilities stay narrow

`dialog:allow-open` replaces the now-unused `dialog:allow-save`; the four window-control permissions above are added. No shell, fs, or generic HTTP permissions exist. CSP is unchanged.

## Consequences

- The engine, its contract, and its tests are untouched; all new logic lives in the tauri-free `settings`/`service` modules and is covered by unit tests that run without a webview.
- Tray behavior depends on Windows shell conventions (left click = restore, right click = menu), which matches the target platform.
- If the webview is hidden, WebView2 may defer timers; the frontend re-syncs from `list_downloads` on `visibilitychange`, and DTO revisions keep ordering honest.
