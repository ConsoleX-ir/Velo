import { memo } from "react";
import { STATE_LABELS, StatePill } from "./ui/badge";
import { ProgressTrack } from "./ui/progress";
import { Spinner } from "./ui/spinner";
import { Tooltip } from "./ui/tooltip";
import { Icon, type IconName } from "./icons";
import {
  destinationDirectory,
  formatBytes,
  formatEta,
  formatRate,
  isTerminal,
  progressPercent,
  speedHistoryPoints,
  type DownloadDto,
} from "../model";

const STATE_ICON: Record<DownloadDto["state"], IconName> = {
  queued: "clock",
  preparing: "globe",
  downloading: "download",
  pausing: "pause",
  paused: "pause",
  cancelling: "cancel",
  finalizing: "check",
  completed: "check",
  failed: "alert",
  cancelled: "cancel",
  interrupted: "refresh",
  retrying: "clock",
};

export type DownloadRowProps = {
  item: DownloadDto;
  speedHistory: number[];
  pending: boolean;
  onTogglePause: () => void;
  onRetry: () => void;
  onCancel: () => void;
  onDismiss: () => void;
  onOpenMenu: (event: React.MouseEvent<HTMLElement>) => void;
};

/**
 * One download as a compact two-zone row: identity + live transfer state on
 * the left, contextual actions on the right. The metadata line carries the
 * bytes, rate, ETA, and a small inline speed sparkline only while a real
 * rate is measured — terminal states never show live values.
 */
export const DownloadRow = memo(function DownloadRow({
  item,
  speedHistory,
  pending,
  onTogglePause,
  onRetry,
  onCancel,
  onDismiss,
  onOpenMenu,
}: DownloadRowProps) {
  const percent = progressPercent(item);
  const terminal = isTerminal(item.state);
  const rate = item.state === "downloading" ? formatRate(item.speedBytesPerSecond) : null;
  const eta = item.state === "downloading" ? formatEta(item.etaSeconds) : null;
  const sparkPoints = speedHistory.length >= 2 && item.state === "downloading"
    ? speedHistoryPoints(speedHistory)
    : null;
  const sparkFirst = sparkPoints?.split(" ")[0].split(",").map(Number) ?? null;
  const sparkLast = sparkPoints?.split(" ").at(-1)?.split(",").map(Number) ?? null;

  const meta = item.state === "retrying"
    ? "Retry scheduled"
    : item.state === "queued"
      ? "Waiting for a transfer slot"
      : item.state === "completed"
        ? `${formatBytes(item.bytesWritten)} saved`
        : percent !== null
          ? `${Math.floor(percent)}% · ${formatBytes(item.bytesWritten)} of ${formatBytes(item.totalBytes ?? 0)}`
          : item.totalBytes === null
            ? `${formatBytes(item.bytesWritten)} · size unknown`
            : formatBytes(item.bytesWritten);

  return (
    <article
      className={`download-row state-${item.state}`}
      onContextMenu={(event) => {
        event.preventDefault();
        onOpenMenu(event);
      }}
    >
      <div className={`row-icon ${terminal ? item.state : "live"}`} aria-hidden="true">
        <Icon name={STATE_ICON[item.state]} size={19} />
      </div>

      <div className="row-main">
        <div className="row-top">
          <span className="row-name" title={item.destination || item.fileName}>{item.fileName}</span>
          <StatePill state={item.state} />
        </div>
        <div className="row-destination">
          <Icon name="folder" size={13} />
          <span className="path-text" title={item.destination}>{destinationDirectory(item.destination)}</span>
        </div>
        {item.state !== "cancelled" && (
          <ProgressTrack
            value={percent ?? (item.state === "completed" ? 100 : null)}
            completed={item.state === "completed"}
            indeterminateWidth={item.state === "queued" ? 0 : 6}
            className="row-progress"
            ariaLabel={`${item.fileName} progress`}
          />
        )}
        <div className="row-meta">
          {item.error && (
            <span
              className={`row-meta-state${item.state === "failed" ? " error" : ""}${
                item.state === "interrupted" || item.state === "retrying" ? " warn" : ""
              }`}
            >
              {item.error}
            </span>
          )}
          <span className="row-meta-dim">{meta}</span>
          {eta && <span className="row-meta-dim">{eta}</span>}
          {item.state === "finalizing" && <span className="row-meta-dim">Syncing to disk…</span>}
          {sparkPoints && (
            <svg className="row-spark" viewBox="0 0 84 20" preserveAspectRatio="none" role="img" aria-label={`Recent speed ${rate ?? "measuring"}`}>
              <polygon className="row-spark-area" points={`${sparkPoints} 84,20 0,20`} />
              <polyline className="row-spark-line" points={sparkPoints} />
              {sparkFirst && <circle className="row-spark-dot" cx={sparkFirst[0]} cy={sparkFirst[1]} r="1.6" />}
              {sparkLast && <circle className="row-spark-dot current" cx={sparkLast[0]} cy={sparkLast[1]} r="1.9" />}
            </svg>
          )}
          {rate && <span className="row-rate">{rate}</span>}
        </div>
      </div>

      <div className="row-actions">
        {(item.state === "downloading" || item.state === "preparing" || item.state === "paused") && (
          <Tooltip label={item.state === "paused" ? "Resume download" : "Pause download"}>
            <button
              type="button"
              className="row-action"
              onClick={onTogglePause}
              disabled={pending}
              aria-label={`${item.state === "paused" ? "Resume" : "Pause"} ${item.fileName}`}
            >
              <Icon name={item.state === "paused" ? "download" : "pause"} size={16} />
            </button>
          </Tooltip>
        )}
        {!terminal && item.state !== "finalizing" && (
          <button
            type="button"
            className="row-action cancel"
            onClick={onCancel}
            disabled={pending}
            aria-label={`Cancel ${item.fileName}`}
          >
            {pending ? <Spinner /> : <Icon name="close" size={16} />}
          </button>
        )}
        {(item.state === "failed" || item.state === "cancelled" || item.state === "interrupted") && (
          <button type="button" className="row-action" onClick={onRetry} disabled={pending} aria-label={`Retry ${item.fileName}`}>
            <Icon name="refresh" size={16} />
          </button>
        )}
        {terminal && (
          <button type="button" className="row-action" onClick={onDismiss} disabled={pending} aria-label={`Remove ${item.fileName} from the list`}>
            {pending ? <Spinner /> : <Icon name="trash" size={16} />}
          </button>
        )}
        <span className="sr-only">{STATE_LABELS[item.state]}</span>
      </div>
    </article>
  );
});
