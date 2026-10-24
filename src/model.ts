export type DownloadState =
  | "queued"
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
};

export const APP_VERSION = "0.5.0";

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
      return items.filter((item) => item.state === "downloading" || item.state === "preparing" || item.state === "pausing" || item.state === "paused" || item.state === "cancelling" || item.state === "finalizing" || item.state === "retrying");
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
