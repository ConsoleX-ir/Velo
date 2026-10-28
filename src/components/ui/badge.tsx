import type { DownloadState } from "../../model";

/**
 * Single source of truth for human-readable download state labels.
 * Kept in sync with the engine states emitted over `download-updated`.
 */
export const STATE_LABELS: Record<DownloadState, string> = {
  queued: "Queued",
  scheduled: "Scheduled",
  preparing: "Preparing",
  downloading: "Downloading",
  pausing: "Pausing",
  paused: "Paused",
  cancelling: "Cancelling",
  finalizing: "Finishing",
  completed: "Completed",
  failed: "Failed",
  cancelled: "Cancelled",
  interrupted: "Interrupted",
  retrying: "Retrying",
};

/**
 * The compact status pill shown on every download card
 * (`.state-pill {state}` + `.pill-dot` dot + label).
 */
export function StatePill({ state }: { state: DownloadState }) {
  return (
    <span className={`state-pill ${state}`}>
      <span className="pill-dot" aria-hidden="true" />
      {STATE_LABELS[state]}
    </span>
  );
}
