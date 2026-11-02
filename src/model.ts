export type DownloadState =
  | "queued"
  | "scheduled"
  | "preparing"
  | "downloading"
  | "pausing"
  | "paused"
  | "cancelling"
  | "finalizing"
  | "completed"
  | "failed"
  | "cancelled"
  | "interrupted"
  | "retrying";

export type DownloadDto = {
  id: string;
  generation: number;
  revision: number;
  order: number;
  fileName: string;
  /** Absolute destination path the transfer writes to. */
  destination: string;
  state: DownloadState;
  bytesWritten: number;
  totalBytes: number | null;
  speedBytesPerSecond: number | null;
  etaSeconds: number | null;
  error: string | null;
  scheduledAtUnixMs?: number | null;
};

export type DownloadView = "downloads" | "queue" | "completed" | "settings";
export type Filter = "all" | "active" | "queued" | "paused" | "completed" | "failed";
export type SpeedHistory = { sampledAt: number | null; samples: number[] };

/** Snapshot of the persisted application settings (see src-tauri settings.rs). */
export type SettingsDto = {
  /** The user's saved default download directory, when one was chosen. */
  defaultDownloadDir: string | null;
  /** Whether the saved directory still exists and is writable right now. */
  storedDirValid: boolean;
  /** The operating system's Downloads folder, when resolvable. */
  osDefaultDownloadDir: string | null;
  /** Where downloads go without an override: saved default while valid, else OS folder. */
  effectiveDownloadDir: string | null;
  globalSpeedLimitBytesPerSecond?: number | null;
  /** Whether closing the main window hides it to the tray or exits the app. */
  closeBehavior: "minimize_to_tray" | "exit_application";
  /** Whether Velo is registered to launch at system startup. */
  launchAtStartup: boolean;
  /** Whether native notifications are sent for terminal download events. */
  downloadNotifications: boolean;
};

export const APP_VERSION = "0.7.0";

export const SPEED_SAMPLE_INTERVAL_MS = 1_000;
export const SPEED_HISTORY_LIMIT = 36;

export const emptySpeedHistory = (): SpeedHistory => ({ sampledAt: null, samples: [] });

export function recordSpeedSample(
  current: SpeedHistory,
  state: DownloadState,
  rate: number | null,
  now: number,
): SpeedHistory {
  if (isTerminal(state)) return current.samples.length ? emptySpeedHistory() : current;
  if (state !== "downloading" || rate === null || !Number.isFinite(rate) || rate < 0) return current;
  if (current.sampledAt !== null && now - current.sampledAt < SPEED_SAMPLE_INTERVAL_MS) return current;
  return {
    sampledAt: now,
    samples: [...current.samples, rate].slice(-SPEED_HISTORY_LIMIT),
  };
}

/** Points for a small inline sparkline (10:1 aspect by default). */
export function speedHistoryPoints(samples: number[], width = 84, height = 20): string {
  if (!samples.length || width <= 0 || height <= 0) return "";
  const valid = samples.map((value) => Math.max(0, Number.isFinite(value) ? value : 0));
  const max = Math.max(...valid);
  const pad = 2;
  return valid.map((value, index) => {
    const x = valid.length === 1 ? width / 2 : (index / (valid.length - 1)) * width;
    const y = height - pad - (max > 0 ? (value / max) * (height - pad * 2) : 0);
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  }).join(" ");
}

export function isTerminal(state: DownloadState): boolean {
  return state === "completed" || state === "failed" || state === "cancelled" || state === "interrupted";
}

/* ---------------------------------------------------------------- file identity */

export type FileTone = "archive" | "disk" | "exec" | "media" | "image" | "doc" | "code" | "generic";

export type FileTypeInfo = {
  /** Short uppercase code rendered inside the card's type tile (e.g. "ZIP"). */
  code: string;
  tone: FileTone;
};

const EXT_TONES: Record<string, FileTone> = {
  zip: "archive", "7z": "archive", rar: "archive", gz: "archive", bz2: "archive", xz: "archive", tar: "archive", zst: "archive",
  iso: "disk", img: "disk", dmg: "disk", vhd: "disk", vhdx: "disk", deb: "disk", rpm: "disk",
  exe: "exec", msi: "exec", msix: "exec", appx: "exec", apk: "exec", jar: "exec",
  mp4: "media", mkv: "media", mov: "media", avi: "media", webm: "media", wmv: "media",
  mp3: "media", flac: "media", wav: "media", m4a: "media", ogg: "media", opus: "media",
  png: "image", jpg: "image", jpeg: "image", gif: "image", webp: "image", bmp: "image", tiff: "image", svg: "image", psd: "image",
  pdf: "doc", doc: "doc", docx: "doc", xls: "doc", xlsx: "doc", ppt: "doc", pptx: "doc", txt: "doc", md: "doc", epub: "doc", mobi: "doc", csv: "doc",
  js: "code", mjs: "code", ts: "code", tsx: "code", jsx: "code", py: "code", rs: "code", go: "code",
  json: "code", html: "code", css: "code", sh: "code", ps1: "code", lua: "code", c: "code", cpp: "code", cs: "code", java: "code",
};

/**
 * File identity for the download card's type tile: a short uppercase
 * extension code plus a tone class. Falls back to a generic FILE glyph when
 * the name carries no usable extension.
 */
export function fileTypeInfo(fileName: string): FileTypeInfo {
  const match = /\.([a-z0-9]{1,6})$/i.exec(fileName.trim());
  if (!match) return { code: "FILE", tone: "generic" };
  const ext = match[1].toLowerCase();
  const tone = EXT_TONES[ext] ?? "generic";
  const code = ext.length <= 4 ? ext.toUpperCase() : `${ext.slice(0, 4).toUpperCase()}`;
  return { code, tone };
}

export function mergeDownloads(current: DownloadDto[], incoming: DownloadDto[]): DownloadDto[] {
  const byHandle = new Map(current.map((item) => [`${item.id}:${item.generation}`, item]));
  for (const item of incoming) {
    const key = `${item.id}:${item.generation}`;
    const present = byHandle.get(key);
    if (!present || item.revision > present.revision) byHandle.set(key, item);
  }
  return [...byHandle.values()].sort((a, b) => a.order - b.order);
}

export function selectDownloads(items: DownloadDto[], view: DownloadView, filter: Filter): DownloadDto[] {
  if (view === "queue") return items.filter((item) => item.state === "queued");
  if (view === "completed") return items.filter((item) => item.state === "completed");
  if (view !== "downloads") return [];
  switch (filter) {
    case "active":
      return items.filter((item) => item.state === "downloading" || item.state === "preparing" || item.state === "pausing" || item.state === "cancelling" || item.state === "finalizing" || item.state === "retrying");
    case "queued":
      return items.filter((item) => item.state === "queued");
    case "paused":
      return items.filter((item) => item.state === "paused");
    case "completed":
      return items.filter((item) => item.state === "completed");
    case "failed":
      return items.filter((item) => item.state === "failed" || item.state === "cancelled" || item.state === "interrupted");
    default:
      return items;
  }
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value >= 100 ? 0 : 1)} ${units[unit]}`;
}

export function progressPercent(item: DownloadDto): number | null {
  if (item.totalBytes === null || item.totalBytes === 0) return null;
  return Math.min(100, Math.max(0, (item.bytesWritten / item.totalBytes) * 100));
}

export function formatRate(rate: number | null): string | null {
  return rate !== null && Number.isFinite(rate) && rate >= 0 ? `${formatBytes(rate)}/s` : null;
}

export function formatEta(seconds: number | null): string | null {
  if (seconds === null || !Number.isFinite(seconds) || seconds < 0) return null;
  if (seconds < 60) return `${Math.ceil(seconds)} sec left`;
  const minutes = Math.ceil(seconds / 60);
  if (minutes < 60) return `${minutes} min left`;
  const hours = Math.floor(minutes / 60);
  return `${hours} hr ${minutes % 60} min left`;
}

/**
 * The containing directory of a download destination, for row metadata
 * ("C:\\Users\\me\\Downloads\\file.zip" -> "C:\\Users\\me\\Downloads").
 */
export function destinationDirectory(destination: string): string {
  const trimmed = destination.replace(/[\\/]+$/, "");
  const slash = Math.max(trimmed.lastIndexOf("\\"), trimmed.lastIndexOf("/"));
  if (slash <= 0) return trimmed;
  return trimmed.slice(0, slash);
}

/* ---------------------------------------------------------------- URL inspection */

export type InspectMetadata = {
  /** Final URL after redirects. */
  effectiveUrl: string;
  /** HTTP status of the response the metadata was drawn from. */
  status: number;
  /** Suggested file name, from `Content-Disposition` or the URL path. */
  suggestedFilename: string | null;
  /** `Content-Length`, when the server declares one. */
  contentLength: number | null;
  /** `Content-Type`, when present. */
  contentType: string | null;
  /** Whether the server advertises byte-range support. `null` when unknown. */
  acceptsRanges: boolean | null;
  /** `Server` header, when present. */
  server: string | null;
};

export type InspectState = "idle" | "loading" | "done" | "error";

export function validateUrl(value: string): string | null {
  try {
    const url = new URL(value.trim());
    if (url.protocol !== "http:" && url.protocol !== "https:") return "Use an HTTP or HTTPS link.";
    if (!url.hostname || url.username || url.password) return "Enter a valid URL without embedded credentials.";
    return null;
  } catch {
    return "Enter a complete URL, including https:// or http://.";
  }
}

export function suggestedFileName(value: string): string {
  try {
    const url = new URL(value.trim());
    const segment = url.pathname.split("/").filter(Boolean).at(-1);
    const decoded = segment ? decodeURIComponent(segment) : "download";
    const safe = decoded.replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").replace(/[. ]+$/g, "");
    return safe && safe !== "." && safe !== ".." ? safe.slice(0, 120) : "download";
  } catch {
    return "download";
  }
}
