import { Icon, type IconName } from "./icons";
import type { DownloadDto, DownloadView } from "../model";

const NAV: { id: DownloadView; label: string; icon: IconName }[] = [
  { id: "downloads", label: "Downloads", icon: "download" },
  { id: "queue", label: "Queue", icon: "queue" },
  { id: "completed", label: "Completed", icon: "check" },
  { id: "settings", label: "Settings", icon: "settings" },
];

const ACTIVE_STATES = new Set(["preparing", "downloading", "pausing", "cancelling", "finalizing", "retrying"]);

type SidebarProps = {
  view: DownloadView;
  onView: (view: DownloadView) => void;
  downloads: DownloadDto[];
};

export function Sidebar({ view, onView, downloads }: SidebarProps) {
  const activeCount = downloads.filter((item) => ACTIVE_STATES.has(item.state)).length;
  const queueCount = downloads.filter((item) => item.state === "queued").length;
  const completedCount = downloads.filter((item) => item.state === "completed").length;
  const badgeFor = (id: DownloadView) =>
    id === "queue" ? queueCount : id === "completed" ? completedCount : 0;

  return (
    <aside className="sidebar">
      <div className="side-section-label">Library</div>
      <nav className="side-nav" aria-label="Main navigation">
        {NAV.map((item) => {
          const badge = badgeFor(item.id);
          const selected = view === item.id;
          return (
            <button
              key={item.id}
              type="button"
              className={`side-nav-item${selected ? " selected" : ""}`}
              onClick={() => onView(item.id)}
              aria-current={selected ? "page" : undefined}
            >
              <Icon name={item.icon} size={17} />
              <span>{item.label}</span>
              {badge > 0 && <span className="side-nav-badge">{badge}</span>}
              {item.id === "downloads" && activeCount > 0 && <span className="side-nav-live" aria-label={`${activeCount} active`} />}
            </button>
          );
        })}
      </nav>

      <div className="side-spacer" />

      <div className="side-summary" aria-label="Transfer summary">
        <div className="side-summary-title">
          <span className="live-dot" aria-hidden="true" />
          Transfers
        </div>
        <dl className="side-summary-grid">
          <div><dt>Active</dt><dd>{activeCount}</dd></div>
          <div><dt>Queued</dt><dd>{queueCount}</dd></div>
          <div><dt>Done</dt><dd>{completedCount}</dd></div>
        </dl>
      </div>

      <div className="side-note">
        <Icon name="info" size={14} />
        <span>Closing the window keeps downloads running in the tray.</span>
      </div>
    </aside>
  );
}
