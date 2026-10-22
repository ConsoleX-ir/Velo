import { useEffect, useMemo, useRef, useState, type FormEvent, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import {
  formatBytes,
  formatEta,
  formatRate,
  isTerminal,
  mergeDownloads,
  progressPercent,
  selectDownloads,
  suggestedFileName,
  validateUrl,
  type DownloadDto,
  type DownloadView,
  type Filter,
} from "./model";

const navItems: { id: DownloadView; label: string; icon: IconName }[] = [
  { id: "downloads", label: "Downloads", icon: "download" },
  { id: "queue", label: "Queue", icon: "queue" },
  { id: "completed", label: "Completed", icon: "check" },
  { id: "categories", label: "Categories", icon: "grid" },
  { id: "settings", label: "Settings", icon: "settings" },
];

type IconName = "download" | "queue" | "check" | "grid" | "settings" | "plus" | "search" | "sun" | "moon" | "folder" | "close" | "globe" | "arrow" | "spark" | "pause" | "alert" | "clock" | "cancel" | "info";

function Icon({ name, size = 18 }: { name: IconName; size?: number }) {
  const paths: Record<IconName, ReactNode> = {
    download: <><path d="M12 3v11"/><path d="m7 10 5 5 5-5"/><path d="M5 20h14"/></>,
    queue: <><path d="M4 6h10M4 12h10M4 18h6"/><path d="m17 15 3 3-3 3"/><path d="M20 18h-7"/></>,
    check: <><circle cx="12" cy="12" r="9"/><path d="m8 12 2.5 2.5L16 9"/></>,
    grid: <><rect x="4" y="4" width="6" height="6" rx="1"/><rect x="14" y="4" width="6" height="6" rx="1"/><rect x="4" y="14" width="6" height="6" rx="1"/><rect x="14" y="14" width="6" height="6" rx="1"/></>,
    settings: <><circle cx="12" cy="12" r="3"/><path d="m19.4 15 .1.1a1.7 1.7 0 0 1-2.4 2.4l-.1-.1a1.7 1.7 0 0 0-2.9 1.2v.2a1.7 1.7 0 0 1-3.4 0v-.2A1.7 1.7 0 0 0 8 17.4l-.1.1a1.7 1.7 0 0 1-2.4-2.4l.1-.1a1.7 1.7 0 0 0-1.2-2.9H4.2a1.7 1.7 0 0 1 0-3.4h.2A1.7 1.7 0 0 0 5.6 6.8l-.1-.1a1.7 1.7 0 0 1 2.4-2.4l.1.1a1.7 1.7 0 0 0 2.9-1.2V3a1.7 1.7 0 0 1 3.4 0v.2a1.7 1.7 0 0 0 2.9 1.2l.1-.1a1.7 1.7 0 0 1 2.4 2.4l-.1.1a1.7 1.7 0 0 0 1.2 2.9h.2a1.7 1.7 0 0 1 0 3.4h-.2a1.7 1.7 0 0 0-1.3 1.9Z"/></>,
    plus: <><path d="M12 5v14M5 12h14"/></>,
    search: <><circle cx="10.8" cy="10.8" r="6.8"/><path d="m16 16 4 4"/></>,
    sun: <><circle cx="12" cy="12" r="4"/><path d="M12 2v2m0 16v2M4.9 4.9l1.4 1.4m11.4 11.4 1.4 1.4M2 12h2m16 0h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/></>,
    moon: <path d="M20.5 14.2A8.5 8.5 0 0 1 9.8 3.5 8.8 8.8 0 1 0 20.5 14.2Z"/>,
    folder: <><path d="M3 6.5A1.5 1.5 0 0 1 4.5 5H10l2 2h7.5A1.5 1.5 0 0 1 21 8.5v9a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17.5Z"/><path d="M3 9h18"/></>,
    close: <><path d="m6 6 12 12M18 6 6 18"/></>,
    globe: <><circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18"/></>,
    arrow: <><path d="M5 12h14M13 6l6 6-6 6"/></>,
    spark: <><path d="m12 3 1.6 6.4L20 11l-6.4 1.6L12 19l-1.6-6.4L4 11l6.4-1.6Z"/><path d="m19 15 .8 2.2L22 18l-2.2.8L19 21l-.8-2.2L16 18l2.2-.8Z"/></>,
    pause: <><path d="M8 5v14M16 5v14"/></>,
    alert: <><path d="M10.3 4.3 2.7 17.5A1.7 1.7 0 0 0 4.2 20h15.6a1.7 1.7 0 0 0 1.5-2.5L13.7 4.3a2 2 0 0 0-3.4 0Z"/><path d="M12 9v4m0 3h.01"/></>,
    clock: <><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/></>,
    cancel: <><circle cx="12" cy="12" r="9"/><path d="m9 9 6 6m0-6-6 6"/></>,
    info: <><circle cx="12" cy="12" r="9"/><path d="M12 11v5m0-8h.01"/></>,
  };
  return <svg aria-hidden="true" width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">{paths[name]}</svg>;
}

function errorText(error: unknown): string {
  if (typeof error === "object" && error !== null && "message" in error) return String(error.message);
  return typeof error === "string" ? error : "Something went wrong. Please try again.";
}

export default function App() {
  const [downloads, setDownloads] = useState<DownloadDto[]>([]);
  const [view, setView] = useState<DownloadView>("downloads");
  const [filter, setFilter] = useState<Filter>("all");
  const [query, setQuery] = useState("");
  const [dialogOpen, setDialogOpen] = useState(false);
  const [ready, setReady] = useState(false);
  const [engineConnected, setEngineConnected] = useState(false);
  const [connectionError, setConnectionError] = useState<string | null>(null);
  const [theme, setTheme] = useState(() => localStorage.getItem("velo-theme") === "light" ? "light" : "dark");
  const [reducedMotion, setReducedMotion] = useState(() => localStorage.getItem("velo-motion") === "reduced");
  const [pending, setPending] = useState<Set<string>>(() => new Set());
  const searchRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    localStorage.setItem("velo-theme", theme);
  }, [theme]);
  useEffect(() => {
    document.documentElement.dataset.motion = reducedMotion ? "reduced" : "system";
    localStorage.setItem("velo-motion", reducedMotion ? "reduced" : "system");
  }, [reducedMotion]);

  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        const stopListening = await listen<DownloadDto>("download-updated", (event) => {
          if (alive) setDownloads((current) => mergeDownloads(current, [event.payload]));
        });
        if (!alive) stopListening();
        else unlisten = stopListening;
        const initial = await invoke<DownloadDto[]>("list_downloads");
        if (alive) {
          setDownloads((current) => mergeDownloads(current, initial));
          setEngineConnected(true);
          setReady(true);
          setConnectionError(null);
        }
      } catch (error) {
        if (alive) {
          setEngineConnected(false);
          setConnectionError(errorText(error));
          setReady(true);
        }
      }
    })();
    return () => {
      alive = false;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    const onShortcut = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        searchRef.current?.focus();
      }
    };
    window.addEventListener("keydown", onShortcut);
    return () => window.removeEventListener("keydown", onShortcut);
  }, []);

  const visibleDownloads = useMemo(() => {
    let items = selectDownloads(downloads, view, filter);
    const normalizedQuery = query.trim().toLocaleLowerCase();
    if (normalizedQuery) items = items.filter((item) => item.fileName.toLocaleLowerCase().includes(normalizedQuery));
    return items;
  }, [downloads, filter, query, view]);
  const activeCount = downloads.filter((item) => ["preparing", "downloading", "cancelling", "finalizing"].includes(item.state)).length;
  const queueCount = downloads.filter((item) => item.state === "queued").length;
  const completedCount = downloads.filter((item) => item.state === "completed").length;

  async function cancelDownload(item: DownloadDto) {
    const key = `${item.id}:${item.generation}`;
    setPending((current) => new Set(current).add(key));
    try {
      await invoke("cancel_download", { id: item.id, generation: item.generation });
    } catch (error) {
      setConnectionError(errorText(error));
    } finally {
      setPending((current) => {
        const next = new Set(current);
        next.delete(key);
        return next;
      });
    }
  }

  async function dismissDownload(item: DownloadDto) {
    const key = `${item.id}:${item.generation}`;
    setPending((current) => new Set(current).add(key));
    try {
      await invoke("acknowledge_download", { id: item.id, generation: item.generation });
      setDownloads((current) => current.filter((candidate) => candidate.id !== item.id || candidate.generation !== item.generation));
    } catch (error) {
      setConnectionError(errorText(error));
    } finally {
      setPending((current) => {
        const next = new Set(current);
        next.delete(key);
        return next;
      });
    }
  }

  const title = view === "downloads" ? "Your downloads" : navItems.find((item) => item.id === view)?.label ?? "Downloads";
  const subtitle = view === "completed"
    ? "Completed in this session. Download history is not saved between launches."
    : view === "queue"
      ? "Jobs waiting for one of the available transfer slots."
      : view === "categories"
        ? "Category organization is not part of this release."
        : view === "settings"
          ? "Make Velo feel right for your workspace."
          : "A clear view of every transfer, from the first byte to the finish.";

  return (
    <div className="app-shell">
      <div className="ambient ambient-one" aria-hidden="true" />
      <div className="ambient ambient-two" aria-hidden="true" />
      <aside className="sidebar">
        <div className="brand-lockup">
          <div className="brand-mark"><span /><span /><span /></div>
          <div><span className="brand-name">console<span>x</span></span><span className="brand-product">VELO</span></div>
        </div>
        <div className="sidebar-section-label">WORKSPACE</div>
        <nav className="main-nav" aria-label="Main navigation">
          {navItems.map((item) => (
            <button key={item.id} className={`nav-item ${view === item.id ? "selected" : ""}`} onClick={() => setView(item.id)} aria-current={view === item.id ? "page" : undefined}>
              <Icon name={item.icon} /> <span>{item.label}</span>
              {item.id === "queue" && queueCount > 0 && <span className="nav-count">{queueCount}</span>}
            </button>
          ))}
        </nav>
        <div className="sidebar-spacer" />
        <div className="side-activity">
          <div className="side-activity-top"><span className="live-dot" /> SESSION ACTIVITY</div>
          <div className="activity-number">{activeCount.toString().padStart(2, "0")} <span>active</span></div>
          <div className="activity-track"><span style={{ width: `${Math.min(100, activeCount * 25)}%` }} /></div>
          <div className="activity-meta"><span>{queueCount} queued</span><span>{completedCount} done</span></div>
        </div>
        <button className="profile-row" onClick={() => setView("settings")} aria-label="Open settings">
          <div className="profile-avatar"><Icon name="spark" size={17} /></div>
          <div className="profile-copy"><strong>Local workspace</strong><span>Private by design</span></div>
          <span className="profile-status" />
        </button>
      </aside>

      <main className="main-area">
        <header className="topbar">
          <div className="breadcrumb"><span>Velo</span><span className="crumb-slash">/</span><strong>{title}</strong></div>
          <div className="topbar-actions">
            <label className="search-box">
              <Icon name="search" size={17} />
              <input ref={searchRef} aria-label="Search downloads" placeholder="Search downloads" value={query} onChange={(event) => setQuery(event.target.value)} />
              <kbd>CTRL K</kbd>
            </label>
            <button className="theme-quick" aria-label={`Switch to ${theme === "dark" ? "light" : "dark"} theme`} onClick={() => setTheme(theme === "dark" ? "light" : "dark")}><Icon name={theme === "dark" ? "sun" : "moon"} /></button>
            <button className="primary-button top-add" onClick={() => setDialogOpen(true)}><Icon name="plus" /> <span>Add download</span></button>
          </div>
        </header>

        <section className="workspace">
          {connectionError && <div className="connection-banner" role="alert"><Icon name="alert" size={17} /><span>{connectionError}</span><button onClick={() => setConnectionError(null)} aria-label="Dismiss message"><Icon name="close" size={16} /></button></div>}
          {view === "settings" ? (
            <Settings theme={theme} setTheme={setTheme} reducedMotion={reducedMotion} setReducedMotion={setReducedMotion} />
          ) : view === "categories" ? (
            <section className="info-view">
              <div className="eyebrow"><Icon name="grid" size={15} /> ORGANIZE</div>
              <h1>Categories, <em>when you need them.</em></h1>
              <p>Downloads are kept in this session only. Persistent categories and saved history are not available yet, so Velo won’t create empty or misleading category lists.</p>
              <button className="secondary-button" onClick={() => setView("downloads")}>Back to downloads <Icon name="arrow" size={16} /></button>
            </section>
          ) : (
            <>
              <div className="page-heading">
                <div>
                  <div className="eyebrow"><span className="eyebrow-line" /> DOWNLOAD CENTER</div>
                  <h1>{title === "Your downloads" ? <>Move at <em>your speed.</em></> : title}</h1>
                  <p>{subtitle}</p>
                </div>
                <div className="heading-stats">
                  <div><span className="stat-dot cyan" /> <strong>{activeCount.toString().padStart(2, "0")}</strong><small>ACTIVE</small></div>
                  <div><span className="stat-dot violet" /> <strong>{queueCount.toString().padStart(2, "0")}</strong><small>IN QUEUE</small></div>
                  <div><span className="stat-dot green" /> <strong>{completedCount.toString().padStart(2, "0")}</strong><small>COMPLETED</small></div>
                </div>
              </div>

              {view === "downloads" && <div className="filter-row" role="group" aria-label="Filter downloads">
                {(["all", "active", "queued", "completed", "failed"] as Filter[]).map((value) => <button key={value} className={`filter-chip ${filter === value ? "active" : ""}`} onClick={() => setFilter(value)}>{value === "failed" ? "Failed / cancelled" : value[0].toUpperCase() + value.slice(1)}</button>)}
                <span className="filter-spacer" />
                <span className="session-tag"><span /> SESSION ONLY</span>
              </div>}

              {!ready ? <div className="loading-state"><span className="loading-ring" /> Connecting to the local download engine…</div> : visibleDownloads.length === 0 ? (
                view === "downloads" && filter === "all" && downloads.length === 0 && !query ? <EmptyState onAdd={() => setDialogOpen(true)} /> : <div className="filtered-empty"><div className="filtered-empty-icon"><Icon name="search" size={22} /></div><strong>{query ? "No matching downloads" : view === "completed" ? "Nothing completed yet" : view === "queue" ? "Your queue is clear" : "No items in this filter"}</strong><span>{query ? "Try a different filename or clear your search." : "New activity will appear here as it happens."}</span></div>
              ) : (
                <div className="download-list" aria-live="polite">
                  {visibleDownloads.map((item) => <DownloadCard key={`${item.id}:${item.generation}`} item={item} pending={pending.has(`${item.id}:${item.generation}`)} onCancel={() => void cancelDownload(item)} onDismiss={() => void dismissDownload(item)} />)}
                </div>
              )}
            </>
          )}
        </section>
        <footer className="statusbar"><div><span className={`status-orb ${engineConnected ? "" : "status-orb-offline"}`} /> {!ready ? "Connecting to engine" : engineConnected ? "Engine connected" : "Engine unavailable"} <span className="status-divider">·</span> Local-only transfers</div><div>NO ACCOUNT <span className="status-divider">·</span> NO TELEMETRY <span className="status-divider">·</span> <span className="version-label">VELO 0.2</span></div></footer>
      </main>
      {dialogOpen && <AddDownloadDialog onClose={() => setDialogOpen(false)} onStarted={(item) => { setDownloads((current) => mergeDownloads(current, [item])); setEngineConnected(true); setConnectionError(null); setDialogOpen(false); setView("downloads"); setFilter("all"); }} />}
    </div>
  );
}

function DownloadCard({ item, pending, onCancel, onDismiss }: { item: DownloadDto; pending: boolean; onCancel: () => void; onDismiss: () => void }) {
  const percent = progressPercent(item);
  const terminal = isTerminal(item.state);
  const stateLabel: Record<DownloadDto["state"], string> = {
    queued: "Queued", preparing: "Preparing", downloading: "Downloading", cancelling: "Cancelling", finalizing: "Finishing", completed: "Completed", failed: "Failed", cancelled: "Cancelled",
  };
  const stateIcon: Record<DownloadDto["state"], IconName> = {
    queued: "clock", preparing: "globe", downloading: "download", cancelling: "cancel", finalizing: "pause", completed: "check", failed: "alert", cancelled: "cancel",
  };
  const secondary = item.state === "queued"
    ? "Waiting for an available transfer slot"
    : item.state === "completed"
      ? `${formatBytes(item.bytesWritten)} saved successfully`
      : item.state === "failed"
        ? item.error ?? "The transfer could not be completed"
        : item.state === "cancelled"
          ? "Temporary data was removed"
          : item.totalBytes === null
            ? `${formatBytes(item.bytesWritten)} downloaded · size unknown`
            : `${formatBytes(item.bytesWritten)} of ${formatBytes(item.totalBytes)}`;
  const rate = item.state === "downloading" ? formatRate(item.speedBytesPerSecond) : null;
  const eta = item.state === "downloading" ? formatEta(item.etaSeconds) : null;

  return <article className={`download-card state-${item.state}`}>
    <div className={`file-icon ${terminal ? item.state : "active"}`}><Icon name={stateIcon[item.state]} size={21} /></div>
    <div className="card-main">
      <div className="card-heading"><div className="file-title" title={item.fileName}>{item.fileName}</div><span className={`state-pill ${item.state}`}><span className="pill-dot" />{stateLabel[item.state]}</span></div>
      <div className={`card-secondary ${item.state === "failed" ? "error-copy" : ""}`}>{secondary}</div>
      {item.state !== "cancelled" && <div className={`progress-track ${item.state === "completed" ? "complete" : ""}`}><span style={{ width: `${percent ?? (item.state === "completed" ? 100 : item.state === "queued" ? 0 : 4)}%` }} /></div>}
      <div className="card-meta"><span>{percent === null ? (item.totalBytes === null && item.state === "downloading" ? "SIZE UNKNOWN" : stateLabel[item.state].toUpperCase()) : `${Math.floor(percent)}%`}</span>{rate && <span className="meta-live"><Icon name="download" size={12} /> {rate}</span>}{eta && <span>{eta}</span>}{item.state === "finalizing" && <span>Syncing file to disk</span>}</div>
    </div>
    <div className="card-actions">
      {!terminal && item.state !== "finalizing" && <button className="icon-button cancel-button" onClick={onCancel} disabled={pending} aria-label={`Cancel ${item.fileName}`} title="Cancel download">{pending ? <span className="button-spinner" /> : <Icon name="close" size={17} />}</button>}
      {terminal && <button className="text-action" onClick={onDismiss} disabled={pending}>{pending ? "Removing…" : "Dismiss"}</button>}
    </div>
  </article>;
}

function EmptyState({ onAdd }: { onAdd: () => void }) {
  return <div className="empty-state">
    <div className="motion-wave" aria-hidden="true"><span className="wave wave-a" /><span className="wave wave-b" /><span className="wave wave-c" /><div className="wave-core"><Icon name="download" size={30} /></div><i className="wave-star star-a" /><i className="wave-star star-b" /><i className="wave-star star-c" /></div>
    <div className="empty-eyebrow"><span /> READY WHEN YOU ARE <span /></div>
    <h2>A little more <em>flow.</em></h2>
    <p>Your download space is clear. Add a link and let Velo take it from here.</p>
    <button className="primary-button empty-add" onClick={onAdd}><Icon name="plus" /> Add your first download <Icon name="arrow" size={16} /></button>
    <div className="empty-footnote"><Icon name="globe" size={14} /> HTTP and HTTPS links supported</div>
  </div>;
}

function Settings({ theme, setTheme, reducedMotion, setReducedMotion }: { theme: string; setTheme: (value: string) => void; reducedMotion: boolean; setReducedMotion: (value: boolean) => void }) {
  return <section className="settings-view">
    <div className="eyebrow"><Icon name="settings" size={15} /> PREFERENCES</div>
    <h1>Make it <em>yours.</em></h1>
    <p className="settings-intro">A few thoughtful controls for your Velo workspace.</p>
    <div className="settings-card">
      <div className="settings-item"><div className="settings-icon"><Icon name="sun" /></div><div className="settings-copy"><strong>Appearance</strong><span>Choose a palette designed for your space.</span></div><div className="segmented-control" role="group" aria-label="Theme">
        <button className={theme === "dark" ? "selected" : ""} onClick={() => setTheme("dark")}><Icon name="moon" size={15} /> Dark</button>
        <button className={theme === "light" ? "selected" : ""} onClick={() => setTheme("light")}><Icon name="sun" size={15} /> Light</button>
      </div></div>
      <div className="settings-separator" />
      <div className="settings-item"><div className="settings-icon motion-icon"><Icon name="spark" /></div><div className="settings-copy"><strong>Reduced motion</strong><span>Use a static aurora and remove interface transitions.</span></div><button className={`toggle ${reducedMotion ? "on" : ""}`} role="switch" aria-checked={reducedMotion} aria-label="Reduced motion" onClick={() => setReducedMotion(!reducedMotion)}><span /></button></div>
    </div>
    <div className="privacy-note"><div className="privacy-lock">◇</div><div><strong>Private by design</strong><span>Downloads stay on this device. No account, telemetry, or saved history.</span></div></div>
    <div className="settings-limit"><Icon name="info" size={15} /> User-controlled pause, persistent history, and automatic retry are not available in this release.</div>
  </section>;
}

function AddDownloadDialog({ onClose, onStarted }: { onClose: () => void; onStarted: (download: DownloadDto) => void }) {
  const [url, setUrl] = useState("");
  const [destination, setDestination] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const urlError = url.trim() ? validateUrl(url) : null;
  const previouslyFocused = useRef<HTMLElement | null>(document.activeElement instanceof HTMLElement ? document.activeElement : null);

  useEffect(() => {
    return () => {
      if (previouslyFocused.current?.isConnected) previouslyFocused.current.focus();
    };
  }, []);

  useEffect(() => {
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape" && !busy) onClose(); };
    window.addEventListener("keydown", escape);
    return () => window.removeEventListener("keydown", escape);
  }, [busy, onClose]);

  async function chooseDestination() {
    setError(null);
    try {
      const selected = await save({ defaultPath: suggestedFileName(url) });
      if (selected) setDestination(selected);
    } catch (cause) {
      setError(errorText(cause));
    }
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    const validation = validateUrl(url);
    if (validation) { setError(validation); return; }
    if (!destination) { setError("Choose where to save this download."); return; }
    setBusy(true);
    setError(null);
    try {
      const download = await invoke<DownloadDto>("start_download", { url: url.trim(), destination });
      onStarted(download);
    } catch (cause) {
      setError(errorText(cause));
      setBusy(false);
    }
  }

  return <div className="modal-scrim" onMouseDown={(event) => { if (event.target === event.currentTarget && !busy) onClose(); }}>
    <section className="add-dialog" role="dialog" aria-modal="true" aria-labelledby="add-title" aria-describedby="add-description" onKeyDown={(event) => {
      if (event.key !== "Tab") return;
      const controls = event.currentTarget.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled)");
      if (!controls.length) return;
      const first = controls[0];
      const last = controls[controls.length - 1];
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
    }}>
      <div className="dialog-top"><div className="dialog-icon"><Icon name="plus" size={21} /></div><button className="icon-button dialog-close" onClick={onClose} disabled={busy} aria-label="Close dialog"><Icon name="close" /></button></div>
      <div className="eyebrow"><span className="eyebrow-line" /> NEW TRANSFER</div>
      <h2 id="add-title">Add a <em>download.</em></h2>
      <p className="dialog-intro" id="add-description">Paste a direct HTTP or HTTPS file link to get started.</p>
      <form onSubmit={(event) => void submit(event)}>
        <label className="field-label" htmlFor="download-url">DOWNLOAD URL</label>
        <div className={`input-wrap ${urlError ? "invalid" : ""}`}><Icon name="globe" size={17} /><input autoFocus id="download-url" type="url" placeholder="https://example.com/file.zip" value={url} onChange={(event) => { setUrl(event.target.value); setError(null); }} aria-invalid={Boolean(urlError)} aria-describedby={urlError ? "url-error" : undefined} /></div>
        {urlError && <span className="field-error" id="url-error">{urlError}</span>}
        <label className="field-label destination-label" htmlFor="download-destination">SAVE TO</label>
        <div className="destination-row"><div className={`input-wrap destination-input ${destination ? "has-value" : ""}`}><Icon name="folder" size={17} /><input id="download-destination" readOnly placeholder="Choose a destination…" value={destination} /><button type="button" className="browse-button" onClick={() => void chooseDestination()} disabled={busy}>Browse</button></div></div>
        {error && <div className="dialog-error" role="alert"><Icon name="alert" size={16} /><span>{error}</span></div>}
        <div className="dialog-actions"><button type="button" className="secondary-button" onClick={onClose} disabled={busy}>Cancel</button><button type="submit" className="primary-button" disabled={busy || Boolean(urlError) || !url.trim() || !destination}>{busy ? <><span className="button-spinner" /> Starting…</> : <>Start download <Icon name="arrow" size={16} /></>}</button></div>
        <div className="dialog-trust"><span className="status-orb" /> Transfers are handled locally by the Velo engine</div>
      </form>
    </section>
  </div>;
}
