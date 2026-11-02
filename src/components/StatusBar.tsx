import { APP_VERSION } from "../model";

type StatusBarProps = {
  ready: boolean;
  engineConnected: boolean;
  /** Number of currently active transfers (client-side count). */
  activeCount?: number;
  /** Pre-formatted aggregate rate across downloading items, or null. */
  rateLabel?: string | null;
};

export function StatusBar({ ready, engineConnected, activeCount = 0, rateLabel }: StatusBarProps) {
  const status = !ready ? "Connecting to engine" : engineConnected ? "Engine connected" : "Engine unavailable";
  return (
    <footer className="statusbar">
      <div className="statusbar-left">
        <span className={`status-orb${engineConnected ? "" : " offline"}`} aria-hidden="true" />
        <span>{status}</span>
        <span className="status-divider" aria-hidden="true">·</span>
        <span>Local-only transfers</span>
        {activeCount > 0 && rateLabel && (
          <>
            <span className="status-divider" aria-hidden="true">·</span>
            <span className="status-extra">
              <strong>{rateLabel}</strong> across {activeCount} active
            </span>
          </>
        )}
      </div>
      <div className="statusbar-right">
        <span>No account · No telemetry</span>
        <span className="status-divider" aria-hidden="true">·</span>
        <span className="version-label">Velo {APP_VERSION}</span>
      </div>
    </footer>
  );
}
