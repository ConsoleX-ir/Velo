import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * The integrated window chrome. Tauri runs the window with
 * `decorations: false` (tauri.conf.json), so this bar owns dragging and the
 * real Minimize / Maximize / Close controls through the window API — no
 * visual simulation. Closing is intercepted natively (CloseRequested ->
 * hide), so the X button tucks Velo into the tray while transfers continue.
 */
const appWindow = getCurrentWindow();

function Glyph({ children }: { children: React.ReactNode }) {
  return (
    <svg aria-hidden="true" width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1" strokeLinecap="round">
      {children}
    </svg>
  );
}

export function TitleBar() {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    let alive = true;
    let stopResize: (() => void) | undefined;
    void appWindow
      .onResized(() => {
        void appWindow.isMaximized().then((value) => {
          if (alive) setMaximized(value);
        });
      })
      .then((stop) => {
        stopResize = stop;
        if (!alive) stop();
      });
    void appWindow.isMaximized().then((value) => {
      if (alive) setMaximized(value);
    });
    return () => {
      alive = false;
      stopResize?.();
    };
  }, []);

  return (
    <header className="titlebar">
      <div className="titlebar-brand" data-tauri-drag-region>
        <img src="/velo-mark.svg" alt="" className="titlebar-mark" draggable={false} />
        <span className="titlebar-name" data-tauri-drag-region>ConsoleX Velo</span>
      </div>
      <div className="titlebar-controls">
        <button
          type="button"
          className="titlebar-button"
          aria-label="Minimize window"
          onClick={() => void appWindow.minimize()}
        >
          <Glyph><path d="M1 5.5h8" /></Glyph>
        </button>
        <button
          type="button"
          className="titlebar-button"
          aria-label={maximized ? "Restore window size" : "Maximize window"}
          onClick={() => void appWindow.toggleMaximize()}
        >
          {maximized ? (
            <Glyph>
              <path d="M2.5 1.5h5a1 1 0 0 1 1 1v5" />
              <rect x="1.5" y="3.5" width="5" height="5" rx="1" />
            </Glyph>
          ) : (
            <Glyph><rect x="1.5" y="1.5" width="7" height="7" rx="1" /></Glyph>
          )}
        </button>
        <button
          type="button"
          className="titlebar-button titlebar-close"
          aria-label="Close window — Velo keeps downloading in the tray"
          onClick={() => void appWindow.close()}
        >
          <Glyph><path d="m1.5 1.5 7 7M8.5 1.5l-7 7" /></Glyph>
        </button>
      </div>
    </header>
  );
}
