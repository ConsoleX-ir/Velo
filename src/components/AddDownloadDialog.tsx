import { useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { readText } from "@tauri-apps/plugin-clipboard-manager";
import { Dialog } from "./ui/dialog";
import { Button } from "./ui/button";
import { Spinner } from "./ui/spinner";
import { DateTimeField } from "./ui/date-time-picker";
import { Icon } from "./icons";
import { scheduleDownload, startDownload, inspectUrl } from "../commands";
import {
  suggestedFileName,
  formatBytes,
  validateUrl,
  type DownloadDto,
  type InspectMetadata,
  type InspectState,
  type SettingsDto,
} from "../model";
import { errorText } from "../lib/utils";

export type AddDownloadDialogProps = {
  settings: SettingsDto | null;
  onClose: () => void;
  onStarted: (download: DownloadDto) => void;
  onCommandError: (message: string) => void;
};

/**
 * New-download flow. The destination line always distinguishes the persistent
 * default (from settings) from a per-download override, and the native side
 * composes + validates the final path (start_download receives folder and
 * file name separately).
 *
 * Phase 3 additions:
 *  - The URL field is seeded once from the clipboard when the dialog opens
 *    (never overwriting typed text, never starting a download).
 *  - The user can inspect the URL before committing: a HEAD request with a
 *    bounded Range fallback resolves metadata (final URL, size, type, ranges)
 *    without downloading the body or creating a download job.
 *  - Inspection results are bound to a sequence counter so rapid URL changes
 *    can never surface stale metadata.
 */
export function AddDownloadDialog({ settings, onClose, onStarted, onCommandError }: AddDownloadDialogProps) {
  const [url, setUrl] = useState("");
  const [fileName, setFileName] = useState("");
  const [overrideDir, setOverrideDir] = useState<string | null>(null);
  const [nameEdited, setNameEdited] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [scheduledAt, setScheduledAt] = useState("");
  const [speedLimitKib, setSpeedLimitKib] = useState("");

  /* Inspection state, isolated from download submission. */
  const [inspectState, setInspectState] = useState<InspectState>("idle");
  const [inspection, setInspection] = useState<InspectMetadata | null>(null);
  const [inspectError, setInspectError] = useState<string | null>(null);
  /** Increments on every URL change so in-flight inspections go stale. */
  const inspectSeqRef = useRef(0);

  /** Mirrors the latest URL so the one-shot clipboard seed can't wipe typed text. */
  const urlRef = useRef(url);
  urlRef.current = url;
  const urlError = url.trim() ? validateUrl(url) : null;
  const effectiveDir = overrideDir ?? settings?.effectiveDownloadDir ?? null;
  const defaultIsOverridden = overrideDir !== null;
  const storedDirBroken = Boolean(settings?.defaultDownloadDir && !settings.storedDirValid);
  const dirKnown = useMemo(() => Boolean(effectiveDir), [effectiveDir]);
  const urlSuggested = url.trim() && !urlError ? suggestedFileName(url) : null;
  // An inspected file name (Content-Disposition) is more authoritative than
  // the URL-derived guess, but never replaces text the user has typed.
  const suggested = inspection?.suggestedFilename ?? urlSuggested;

  const resolvedFileName = fileName.trim() || suggested || "";
  const minimumDate = new Date(Date.now() + 60_000);
  const minScheduleMs = minimumDate.getTime();

  /**
   * Seed the URL field once from the clipboard when the dialog opens. Only
   * valid HTTP(S) URLs are accepted, and only when the field is still empty,
   * so typed text is never overwritten. Clipboard failure is silent: the user
   * can still paste or type the link manually.
   */
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const text = await readText();
        if (cancelled) return;
        const trimmed = (text ?? "").trim();
        if (trimmed && !validateUrl(trimmed) && !urlRef.current) {
          setUrl(trimmed);
        }
      } catch {
        // Clipboard access denied or unavailable: proceed without seeding.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  async function chooseFolder() {
    setError(null);
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "Choose where to save this download",
      });
      if (typeof selected === "string" && selected) setOverrideDir(selected);
    } catch (cause) {
      setError(errorText(cause));
    }
  }

  async function inspectLink() {
    const validation = validateUrl(url);
    if (validation) {
      setError(validation);
      return;
    }
    setError(null);
    const seq = ++inspectSeqRef.current;
    setInspection(null);
    setInspectError(null);
    setInspectState("loading");
    try {
      const result = await inspectUrl(url.trim());
      if (seq !== inspectSeqRef.current) return;
      setInspection(result);
      setInspectState("done");
    } catch (cause) {
      if (seq !== inspectSeqRef.current) return;
      setInspectState("error");
      setInspectError(errorText(cause));
    }
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    const validation = validateUrl(url);
    if (validation) {
      setError(validation);
      return;
    }
    if (!resolvedFileName) {
      setError("Enter a file name for this download.");
      return;
    }
    if (!effectiveDir) {
      setError("Choose a folder in Settings or for this download.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const scheduleMs = scheduledAt ? new Date(scheduledAt).getTime() : null;
      const speedKib = speedLimitKib ? Number(speedLimitKib) : null;
      if (speedKib !== null && (!Number.isFinite(speedKib) || speedKib < 1 || speedKib > 1_048_576)) {
        setError("Enter a per-download speed limit from 1 to 1,048,576 KiB/s, or leave it blank.");
        setBusy(false);
        return;
      }
      const speedLimitBytesPerSecond = speedKib === null ? null : Math.round(speedKib * 1024);
      if (scheduleMs !== null && (!Number.isFinite(scheduleMs) || scheduleMs <= Date.now())) {
        setError("Choose a future date and time.");
        setBusy(false);
        return;
      }
      const download = scheduleMs === null
        ? await startDownload(url.trim(), effectiveDir, resolvedFileName, speedLimitBytesPerSecond)
        : await scheduleDownload(url.trim(), effectiveDir, resolvedFileName, scheduleMs, speedLimitBytesPerSecond);
      onStarted(download);
    } catch (cause) {
      const message = errorText(cause);
      setError(message);
      onCommandError(message);
      setBusy(false);
    }
  }

  return (
    <Dialog
      open
      onClose={onClose}
      closeOnEscape={!busy}
      closeOnOutsideClick={!busy}
      labelledBy="add-title"
      describedBy="add-description"
      panelClassName="add-dialog"
    >
      <div className="dialog-top">
        <div className="dialog-icon"><Icon name="plus" size={20} /></div>
        <button className="icon-button dialog-close" onClick={onClose} disabled={busy} aria-label="Close dialog">
          <Icon name="close" />
        </button>
      </div>
      <h2 id="add-title">Add a download</h2>
      <p className="dialog-intro" id="add-description">
        Paste a direct HTTP or HTTPS file link. Velo saves it without overwriting existing files.
      </p>
      <form onSubmit={(event) => void submit(event)}>
        <label className="field-label" htmlFor="download-url">Download link</label>
        <div className={`input-wrap${urlError ? " invalid" : ""}`}>
          <Icon name="globe" size={16} />
          <input
            autoFocus
            id="download-url"
            type="url"
            placeholder="https://example.com/file.zip"
            value={url}
            onChange={(event) => {
              const next = event.target.value;
              setUrl(next);
              setError(null);
              // A changed URL invalidates any displayed inspection so stale
              // metadata can never be reviewed for a different link.
              if (next.trim() !== url.trim()) {
                setInspectState("idle");
                setInspection(null);
                setInspectError(null);
                ++inspectSeqRef.current;
              }
            }}
            aria-invalid={Boolean(urlError)}
            aria-describedby={urlError ? "url-error" : undefined}
          />
        </div>
        {urlError && <span className="field-error" id="url-error">{urlError}</span>}
        <div className={`inspect-row${inspectState === "loading" ? " busy" : ""}`}>
          <Button
            type="button"
            onClick={inspectLink}
            disabled={busy || Boolean(urlError) || !url.trim() || inspectState === "loading"}
            aria-label={inspectState === "loading" ? "Inspection in progress" : "Inspect link metadata"}
          >
            {inspectState === "loading" ? (
              <>
                <Spinner />
                <span>Inspecting…</span>
              </>
            ) : (
              <>
                <Icon name="eye" size={15} />
                <span>Inspect link</span>
              </>
            )}
          </Button>
        </div>
        {inspectState !== "idle" && (
          <div className="form-section">
            <div className="form-section-label micro-label"><Icon name="eye" size={13} />Link inspection</div>
            {inspectState === "loading" ? (
              <div className="inspect-loading" role="status" aria-live="polite">
                <Spinner />
                <span>Inspecting link…</span>
              </div>
            ) : inspectState === "error" ? (
              <div className="dialog-error" role="alert">
                <Icon name="alert" size={15} />
                <span>{inspectError}</span>
              </div>
            ) : inspection && (
              <div className="inspect-grid">
                <span className="inspect-key">Final URL</span>
                <span className="inspect-val">
                  <span className="mono path-text" title={inspection.effectiveUrl ?? ""}>{inspection.effectiveUrl ?? "—"}</span>
                </span>
                <span className="inspect-key">Status</span>
                <span className="inspect-val">{String(inspection.status ?? "—")}</span>
                <span className="inspect-key">File name</span>
                <span className="inspect-val">{inspection.suggestedFilename ?? "—"}</span>
                <span className="inspect-key">Size</span>
                <span className="inspect-val">
                  {inspection.contentLength !== null ? formatBytes(inspection.contentLength) : "unknown"}
                </span>
                <span className="inspect-key">Type</span>
                <span className="inspect-val">{inspection.contentType ?? "unknown"}</span>
                <span className="inspect-key">Byte ranges</span>
                <span className="inspect-val">
                  {inspection.acceptsRanges === true ? "Supported" : inspection.acceptsRanges === false ? "Not supported" : "unknown"}
                </span>
                <span className="inspect-key">Server</span>
                <span className="inspect-val">{inspection.server ?? "unknown"}</span>
              </div>
            )}
          </div>
        )}

        <label className="field-label destination-label" htmlFor="download-name">File name</label>
        <div className="input-wrap">
          <input
            id="download-name"
            type="text"
            placeholder={suggested ?? "file.zip"}
            value={fileName}
            onChange={(event) => {
              setFileName(event.target.value);
              setNameEdited(true);
              setError(null);
            }}
            aria-describedby="download-name-hint"
          />
        </div>
        <span className="field-hint" id="download-name-hint">
          {nameEdited || !suggested ? "Saved into the folder below." : `Suggested from the link: ${suggested}`}
        </span>

        <label className="field-label destination-label">Save to</label>
        <div className="destination-box" data-override={defaultIsOverridden || undefined}>
          <Icon name="folder" size={16} />
          <div className="destination-copy">
            <strong className="path-text">{effectiveDir ?? "No folder available yet"}</strong>
            <span>{defaultIsOverridden ? "This download only — the default stays unchanged" : "Default location from Settings"}</span>
          </div>
          <div className="destination-actions">
            {defaultIsOverridden && (
              <button type="button" className="link-button" onClick={() => setOverrideDir(null)} disabled={busy}>
                Use default
              </button>
            )}
            <button type="button" className="browse-button" onClick={() => void chooseFolder()} disabled={busy}>
              Browse…
            </button>
          </div>
        </div>

        <div className="form-section">
          <div className="form-section-label micro-label"><Icon name="clock" size={13} />Scheduling &amp; limits</div>
          <label className="field-label" htmlFor="download-schedule">Start at (optional)</label>
          <DateTimeField id="download-schedule" value={scheduledAt} minMs={minScheduleMs} disabled={busy} onChange={setScheduledAt} />
          <span className="field-hint">Velo starts the download at this local date and time, including after restart.</span>
          <label className="field-label destination-label" htmlFor="download-speed-limit">Per-download speed limit (optional)</label>
          <div className="input-wrap"><input id="download-speed-limit" type="number" min="1" max="1048576" value={speedLimitKib} onChange={(event) => setSpeedLimitKib(event.target.value)} placeholder="Unlimited" /><span className="input-suffix">KiB/s</span></div>
        </div>
        {storedDirBroken && !defaultIsOverridden && (
          <div className="dialog-warn" role="alert">
            <Icon name="alert" size={15} />
            <span>Your saved default folder is unavailable, so Velo will use the system Downloads folder. Pick “Browse…” to override it here, or change it in Settings.</span>
          </div>
        )}
        {error && (
          <div className="dialog-error" role="alert">
            <Icon name="alert" size={15} />
            <span>{error}</span>
          </div>
        )}
        <div className="dialog-actions">
          <Button variant="secondary" onClick={onClose} disabled={busy}>Cancel</Button>
          <Button variant="primary" type="submit" loading={busy} disabled={Boolean(urlError) || !url.trim() || !resolvedFileName || !dirKnown}>
            {busy ? "Saving…" : <>{scheduledAt ? "Schedule download" : "Start download"} <Icon name="arrow" size={15} /></>}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
