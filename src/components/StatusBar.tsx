import { APP_VERSION } from "../model";

type StatusBarProps = {
  ready: boolean;
  engineConnected: boolean;
};

export function StatusBar({ ready, engineConnected }: StatusBarProps) {
  const status = !ready ? "Connecting to engine" : engineConnected ? "Engine connected" : "Engine unavailable";
  return (
    <footer className="statusbar">
      <div className="statusbar-left">
        <span className={`status-orb${engineConnected ? "" : " offline"}`} aria-hidden="true" />
        <span>{status}</span>
        <span className="status-divider" aria-hidden="true">·</span>
        <span>Local-only transfers</span>
      </div>
      <div className="statusbar-right">
        <span>No account · No telemetry</span>
        <span className="status-divider" aria-hidden="true">·</span>
        <span className="version-label">Velo {APP_VERSION}</span>
      </div>
    </footer>
  );
}
