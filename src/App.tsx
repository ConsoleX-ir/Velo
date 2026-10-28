import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type FormEvent } from "react";
import { listen } from "@tauri-apps/api/event";
import { TitleBar } from "./components/TitleBar";
import { Sidebar } from "./components/Sidebar";
import { StatusBar } from "./components/StatusBar";
import { DownloadRow } from "./components/DownloadRow";
import { ContextMenu, type MenuItemSpec } from "./components/ContextMenu";
import { AddDownloadDialog } from "./components/AddDownloadDialog";
import { Dialog } from "./components/ui/dialog";
import { SettingsView } from "./components/SettingsView";
import { EmptyState, FilteredEmpty } from "./components/ui/empty-state";
import { AILoader } from "./components/ui/ai-loader";
import { DateTimeField } from "./components/ui/date-time-picker";
import { LiquidNavbar } from "./components/ui/liquid-glass-navbar";
import { ShaderBackground } from "./components/ui/manu";
import type { SegmentedOption } from "./components/ui/segmented";
import { ToastProvider, useToast } from "./components/ui/toast";
import { Button } from "./components/ui/button";
import { Icon } from "./components/icons";
import { errorText } from "./lib/utils";
import {
  acknowledgeDownload,
  cancelDownload as cancelDownloadCommand,
  getHistoryNotice,
  getSettings,
  listDownloads,
  pauseDownload,
  resumeDownload,
  retryDownload,
  editDownloadSchedule,
} from "./commands";
import {
  emptySpeedHistory,
  isTerminal,
  mergeDownloads,
  recordSpeedSample,
  selectDownloads,
  type DownloadDto,
  type DownloadView,
  type Filter,
  type SettingsDto,
  type SpeedHistory,
} from "./model";

const NO_SPEED_SAMPLES: number[] = [];

const FILTER_OPTIONS: SegmentedOption<Filter>[] = [
  { value: "all", label: "All" },
  { value: "active", label: "Active" },
  { value: "queued", label: "Queued" },
  { value: "paused", label: "Paused" },
  { value: "completed", label: "Completed" },
  { value: "failed", label: "Failed" },
];

const VIEW_COPY: Record<Exclude<DownloadView, "settings">, { title: string; subtitle: string }> = {
  downloads: { title: "Downloads", subtitle: "Every transfer on this device, from first byte to finish." },
  queue: { title: "Queue", subtitle: "Waiting for one of the available transfer slots." },
  completed: { title: "Completed", subtitle: "Files saved to disk. The list is stored locally." },
};

export default function App() {
  return (
    <ToastProvider>
      <VeloApp />
    </ToastProvider>
  );
}

function VeloApp() {
  const { push } = useToast();
  const [downloads, setDownloads] = useState<DownloadDto[]>([]);
  const [settings, setSettings] = useState<SettingsDto | null>(null);
  const [view, setView] = useState<DownloadView>("downloads");
  const [filter, setFilter] = useState<Filter>("all");
  const [query, setQuery] = useState("");
  const [dialogOpen, setDialogOpen] = useState(false);
  const [ready, setReady] = useState(false);
  const [engineConnected, setEngineConnected] = useState(false);
  const [connectionError, setConnectionError] = useState<string | null>(null);
  const [theme, setTheme] = useState<"dark" | "light">(() => localStorage.getItem("velo-theme") === "light" ? "light" : "dark");
  const [reducedMotion, setReducedMotion] = useState(() => localStorage.getItem("velo-motion") === "reduced");
  const [pending, setPending] = useState<Set<string>>(() => new Set());
  const [speedHistories, setSpeedHistories] = useState<Record<string, SpeedHistory>>({});
  const [menuState, setMenuState] = useState<{ x: number; y: number; item: DownloadDto } | null>(null);
  const [scheduleEdit, setScheduleEdit] = useState<DownloadDto | null>(null);
  const latestRevision = useRef(new Map<string, number>());
  const keyStates = useRef(new Map<string, DownloadDto["state"]>());
  const searchRef = useRef<HTMLInputElement>(null);

  const projectDownload = useCallback(
    (item: DownloadDto) => {
      const key = `${item.id}:${item.generation}`;
      const revision = latestRevision.current.get(key);
      if (revision !== undefined && item.revision <= revision) return;
      const previousState = keyStates.current.get(key);
      latestRevision.current.set(key, item.revision);
      keyStates.current.set(key, item.state);
      setDownloads((current) => mergeDownloads(current, [item]));
      const now = Date.now();
      setSpeedHistories((current) => {
        const before = current[key];
        const history = recordSpeedSample(before ?? emptySpeedHistory(), item.state, item.speedBytesPerSecond, now);
        if (history === before || (!before && history.samples.length === 0)) return current;
        const next = { ...current };
        if (history.samples.length) next[key] = history;
        else delete next[key];
        return next;
      });
      if (item.state === "completed" && previousState !== undefined && !isTerminal(previousState)) {
        push({ title: `Download complete — ${item.fileName}`, kind: "success" });
      }
    },
    [push],
  );

  const forgetDownload = useCallback((id: string, generation: number) => {
    const key = `${id}:${generation}`;
    setDownloads((current) => current.filter((item) => !(item.id === id && item.generation === generation)));
    latestRevision.current.delete(key);
    keyStates.current.delete(key);
    setSpeedHistories((current) => {
      if (!current[key]) return current;
      const next = { ...current };
      delete next[key];
      return next;
    });
  }, []);

  useLayoutEffect(() => {
    document.documentElement.dataset.theme = theme;
    localStorage.setItem("velo-theme", theme);
  }, [theme]);
  useLayoutEffect(() => {
    document.documentElement.dataset.motion = reducedMotion ? "reduced" : "system";
    localStorage.setItem("velo-motion", reducedMotion ? "reduced" : "system");
  }, [reducedMotion]);

  const refreshSettings = useCallback(async () => {
    try {
      setSettings(await getSettings());
    } catch {
      // Settings are not critical for rendering the list; the Add dialog
      // surfaces the missing folder at use time.
    }
  }, []);

  useEffect(() => {
    let alive = true;
    const unlisteners: Array<() => void> = [];
    void (async () => {
      try {
        const stopUpdated = await listen<DownloadDto>("download-updated", (event) => {
          if (alive) projectDownload(event.payload);
        });
        unlisteners.push(stopUpdated);
        // A retry replaces its job record with a fresh id; the backend tells
        // the webview to drop the retired row instead of leaving it behind.
        const stopRemoved = await listen<{ id: string; generation: number }>("download-removed", (event) => {
          if (alive) forgetDownload(event.payload.id, event.payload.generation);
        });
        unlisteners.push(stopRemoved);
        const initial = await listDownloads();
        if (alive) {
          initial.forEach(projectDownload);
          setEngineConnected(true);
          setReady(true);
          setConnectionError(null);
          void refreshSettings();
          try {
            const notice = await getHistoryNotice();
            if (alive && typeof notice === "string" && notice.trim().length > 0) {
              push({ title: notice, kind: "info", persistent: true });
            }
          } catch {
            // The recovery notice is best-effort; never block startup on it.
          }
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
      unlisteners.forEach((stop) => stop());
    };
  }, [forgetDownload, projectDownload, push, refreshSettings]);

  // While the window is hidden in the tray the webview may miss or defer
  // events; on return, re-sync from the native list (revisions guard order).
  useEffect(() => {
    if (!engineConnected) return;
    const onVisible = () => {
      if (document.visibilityState !== "visible") return;
      void listDownloads()
        .then((items) => items.forEach(projectDownload))
        .catch(() => undefined);
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  }, [engineConnected, projectDownload]);

  useEffect(() => {
    const onShortcut = (event: KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey)) return;
      const key = event.key.toLowerCase();
      if (key === "k") {
        event.preventDefault();
        searchRef.current?.focus();
      }
      if (key === "n") {
        event.preventDefault();
        setDialogOpen(true);
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

  const activeCount = downloads.filter((item) => ["preparing", "downloading", "pausing", "cancelling", "finalizing", "retrying"].includes(item.state)).length;
  const queueCount = downloads.filter((item) => item.state === "queued").length;
  const completedCount = downloads.filter((item) => item.state === "completed").length;

  async function runCommand(key: string, action: () => Promise<unknown>) {
    setPending((current) => new Set(current).add(key));
    try {
      await action();
    } catch (error) {
      push({ title: errorText(error), kind: "error" });
    } finally {
      setPending((current) => {
        const next = new Set(current);
        next.delete(key);
        return next;
      });
    }
  }

  const togglePause = useCallback(
    (item: DownloadDto) => {
      const paused = item.state === "paused";
      void runCommand(
        `${item.id}:${item.generation}`,
        () => (paused ? resumeDownload(item.id, item.generation) : pauseDownload(item.id, item.generation)),
      );
    },
    // runCommand is stable enough per render; the callbacks below are passed
    // to memoized rows, so keep identities stable per item.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );

  const retry = useCallback(
    (item: DownloadDto) => {
      const key = `${item.id}:${item.generation}`;
      setPending((current) => new Set(current).add(key));
      retryDownload(item.id, item.generation)
        .then((replacement) => {
          latestRevision.current.delete(key);
          keyStates.current.delete(key);
          projectDownload(replacement);
        })
        .catch((error) => push({ title: errorText(error), kind: "error" }))
        .finally(() => {
          setPending((current) => {
            const next = new Set(current);
            next.delete(key);
            return next;
          });
        });
    },
    [projectDownload, push],
  );

  const dismiss = useCallback(
    (item: DownloadDto) => {
      const key = `${item.id}:${item.generation}`;
      setPending((current) => new Set(current).add(key));
      acknowledgeDownload(item.id, item.generation)
        .then(() => forgetDownload(item.id, item.generation))
        .catch((error) => push({ title: errorText(error), kind: "error" }))
        .finally(() => {
          setPending((current) => {
            const next = new Set(current);
            next.delete(key);
            return next;
          });
        });
    },
    [forgetDownload, push],
  );

  const openMenu = useCallback((item: DownloadDto, event: React.MouseEvent<HTMLElement>) => {
    setMenuState({ x: event.clientX, y: event.clientY, item });
  }, []);

  const cancel = useCallback(
    (item: DownloadDto) => {
      void runCommand(`${item.id}:${item.generation}`, () => cancelDownloadCommand(item.id, item.generation));
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );

  function menuItemsFor(item: DownloadDto): MenuItemSpec[] {
    const key = `${item.id}:${item.generation}`;
    const busy = pending.has(key);
    const items: MenuItemSpec[] = [];
    if (item.state === "paused") {
      items.push({ id: "resume", label: "Resume", icon: <Icon name="download" size={15} />, disabled: busy, onSelect: () => togglePause(item) });
    } else if (item.state === "downloading" || item.state === "preparing") {
      items.push({ id: "pause", label: "Pause", icon: <Icon name="pause" size={15} />, disabled: busy, onSelect: () => togglePause(item) });
    }
    if (item.state === "scheduled") {
      items.push({ id: "edit-schedule", label: "Edit schedule", icon: <Icon name="clock" size={15} />, disabled: busy, onSelect: () => setScheduleEdit(item) });
    }
    if (item.state === "failed" || item.state === "cancelled" || item.state === "interrupted") {
      items.push({ id: "retry", label: "Retry", icon: <Icon name="refresh" size={15} />, disabled: busy, onSelect: () => retry(item) });
    }
    if (!isTerminal(item.state) && item.state !== "finalizing") {
      items.push({
        id: "cancel",
        label: item.state === "retrying" ? "Cancel automatic retry" : "Cancel download",
        icon: <Icon name="close" size={15} />,
        danger: true,
        disabled: busy,
        onSelect: () => cancel(item),
      });
    }
    if (isTerminal(item.state)) {
      items.push({ id: "dismiss", label: "Remove from list", icon: <Icon name="trash" size={15} />, disabled: busy, onSelect: () => dismiss(item) });
    }
    return items;
  }

  const showFilters = view === "downloads";
  const listEmpty = visibleDownloads.length === 0;
  const firstRun = view === "downloads" && filter === "all" && downloads.length === 0 && !query;

  return (
    <div className="app-shell">
      {!reducedMotion && <ShaderBackground className="shader-backdrop" />}
      <TitleBar />
      <div className="app-body">
        <Sidebar view={view} onView={setView} downloads={downloads} />
        <main className="main-area">
          {view === "settings" ? (
            <SettingsView
              settings={settings}
              onSettingsChanged={setSettings}
              onSettingsError={(message) => push({ title: message, kind: "error" })}
              theme={theme}
              setTheme={setTheme}
              reducedMotion={reducedMotion}
              setReducedMotion={setReducedMotion}
            />
          ) : (
            <>
              <div className="page-head">
                <div className="page-titles">
                  <h1>{VIEW_COPY[view].title}</h1>
                  <p>{VIEW_COPY[view].subtitle}</p>
                </div>
                <div className="page-stats" aria-hidden="true">
                  <div><strong>{activeCount}</strong><span>Active</span></div>
                  <div><strong>{queueCount}</strong><span>Queued</span></div>
                  <div><strong>{completedCount}</strong><span>Done</span></div>
                </div>
              </div>

              {showFilters && (
                <div className="toolbar">
                  <LiquidNavbar
                    selectable
                    ariaLabel="Filter downloads"
                    className="filter-pill"
                    tone={theme === "dark" ? "dark" : "light"}
                    reducedMotion={reducedMotion}
                    active={Math.max(0, FILTER_OPTIONS.findIndex((option) => option.value === filter))}
                    onChange={(index) => setFilter(FILTER_OPTIONS[index].value)}
                    items={FILTER_OPTIONS.map(({ label }) => ({ label }))}
                  />
                  <label className="search-box">
                    <Icon name="search" size={15} />
                    <input
                      ref={searchRef}
                      aria-label="Search downloads"
                      placeholder="Search downloads"
                      value={query}
                      onChange={(event) => setQuery(event.target.value)}
                    />
                    <kbd>Ctrl K</kbd>
                  </label>
                  <Button variant="primary" className="toolbar-add" onClick={() => setDialogOpen(true)}>
                    <Icon name="plus" size={15} /> <span>Add download</span>
                  </Button>
                </div>
              )}

              {connectionError && (
                <div className="connection-banner" role="alert">
                  <Icon name="alert" size={16} />
                  <span>{connectionError}</span>
                  <button onClick={() => setConnectionError(null)} aria-label="Dismiss message">
                    <Icon name="close" size={14} />
                  </button>
                </div>
              )}

              {!ready ? (
                <div className="loading-state">
                  <AILoader label="Connecting to the local download engine" variant="dots" reduced={reducedMotion} />
                </div>
              ) : listEmpty ? (
                firstRun ? (
                  <EmptyState
                    title="Nothing downloading yet"
                    description="Paste a link and Velo will fetch it in the background — pausable, resumable, and saved exactly where you choose."
                    action={<Button variant="primary" className="empty-add" onClick={() => setDialogOpen(true)}><Icon name="plus" size={15} /> Add your first download</Button>}
                    footnote={<><Icon name="globe" size={13} /> HTTP and HTTPS links supported · No account needed</>}
                  />
                ) : (
                  <FilteredEmpty
                    icon={<Icon name={query ? "search" : "check"} size={20} />}
                    title={query ? "No matching downloads" : view === "completed" ? "Nothing completed yet" : view === "queue" ? "The queue is clear" : "No items in this filter"}
                    hint={query ? "Try a different file name or clear the search." : "New activity appears here as it happens."}
                  />
                )
              ) : (
                <div className="download-list">
                  {visibleDownloads.map((item) => (
                    <DownloadRow
                      key={`${item.id}:${item.generation}`}
                      item={item}
                      speedHistory={speedHistories[`${item.id}:${item.generation}`]?.samples ?? NO_SPEED_SAMPLES}
                      pending={pending.has(`${item.id}:${item.generation}`)}
                      reducedMotion={reducedMotion}
                      onTogglePause={togglePause}
                      onRetry={retry}
                      onCancel={cancel}
                      onDismiss={dismiss}
                      onOpenMenu={openMenu}
                    />
                  ))}
                </div>
              )}
            </>
          )}
        </main>
      </div>
      <StatusBar ready={ready} engineConnected={engineConnected} />
      {dialogOpen && (
        <AddDownloadDialog
          settings={settings}
          onClose={() => setDialogOpen(false)}
          onStarted={(item) => {
            projectDownload(item);
            setEngineConnected(true);
            setConnectionError(null);
            setDialogOpen(false);
            setView("downloads");
            setFilter("all");
            push({ title: item.state === "scheduled" ? `Download scheduled — ${item.fileName}` : `Download started — ${item.fileName}`, kind: "success" });
          }}
          onCommandError={(message) => push({ title: message, kind: "error" })}
        />
      )}
      {menuState && (
        <ContextMenu
          x={menuState.x}
          y={menuState.y}
          items={menuItemsFor(menuState.item)}
          onClose={() => setMenuState(null)}
        />
      )}
      {scheduleEdit && (
        <ScheduleEditDialog item={scheduleEdit} onClose={() => setScheduleEdit(null)} onSaved={(item) => { projectDownload(item); setScheduleEdit(null); }} onError={(message) => push({ title: message, kind: "error" })} />
      )}
    </div>
  );
}

function toLocalDateTime(value: number | null | undefined): string {
  const date = value ? new Date(value) : new Date(Date.now() + 60_000);
  const pad = (part: number) => String(part).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function ScheduleEditDialog({ item, onClose, onSaved, onError }: { item: DownloadDto; onClose: () => void; onSaved: (item: DownloadDto) => void; onError: (message: string) => void }) {
  const [when, setWhen] = useState(toLocalDateTime(item.scheduledAtUnixMs));
  const [busy, setBusy] = useState(false);
  async function save(event: FormEvent) {
    event.preventDefault();
    const ms = new Date(when).getTime();
    if (!Number.isFinite(ms) || ms <= Date.now()) { onError("Choose a future date and time."); return; }
    setBusy(true);
    try { onSaved(await editDownloadSchedule(item.id, item.generation, ms)); }
    catch (error) { onError(errorText(error)); setBusy(false); }
  }
  return <Dialog open onClose={busy ? () => undefined : onClose} closeOnEscape={!busy} closeOnOutsideClick={!busy} labelledBy="edit-schedule-title" panelClassName="add-dialog">
    <h2 id="edit-schedule-title">Edit scheduled download</h2>
    <p className="dialog-intro">{item.fileName}</p>
    <form onSubmit={(event) => void save(event)}>
      <label className="field-label" htmlFor="edit-schedule-time">Start at</label>
      <DateTimeField id="edit-schedule-time" value={when} minMs={Date.now() + 60_000} disabled={busy} onChange={setWhen} />
      <div className="dialog-actions"><Button variant="secondary" onClick={onClose} disabled={busy}>Cancel</Button><Button variant="primary" type="submit" loading={busy}>Save schedule</Button></div>
    </form>
  </Dialog>;
}
