import { memo } from "react";
import { STATE_LABELS, StatePill } from "./ui/badge";
import { AILoader } from "./ui/ai-loader";
import { ProgressTrack } from "./ui/progress";
import { Spinner } from "./ui/spinner";
import { Tooltip } from "./ui/tooltip";
import { Icon } from "./icons";
import {
  destinationDirectory,
  fileTypeInfo,
  formatBytes,
  formatEta,
  formatRate,
  isTerminal,
  progressPercent,
  speedHistoryPoints,
  type DownloadDto,
} from "../model";

const SPARK_WIDTH = 96;
const SPARK_HEIGHT = 22;

export type DownloadRowProps = {
  item: DownloadDto;
  speedHistory: number[];
  pending: boolean;
  /** In-app reduced-motion preference, applied to the queue indicator. */
  reducedMotion?: boolean;
  onTogglePause: (item: DownloadDto) => void;
  onRetry: (item: DownloadDto) => void;
  onCancel: (item: DownloadDto) => void;
  onDismiss: (item: DownloadDto) => void;
  onOpenMenu: (item: DownloadDto, event: React.MouseEvent<HTMLElement>) => void;
};

/**
 * The signature Velo download card — a machined horizontal hybrid.
 *
 *   [state edge][type tile] [ identity + progress ] [ instrument rail ]
 *                            name · destination     rate/size · spark
 *                            ▓▓▓▓▓░░░ 42% · 8 of 10 eta · actions
 *
 * The instrument rail always shows one primary metric: live rate while
 * moving, the saved size once complete, the percentage while paused, and a
 * quiet dash while merely waiting. The sparkline renders exclusively from
 * real sampled speed data and only while a transfer is actually running —
 * terminal states never present speed.
 */
export const DownloadRow = memo(function DownloadRow({
  item,
  speedHistory,
  pending,
  reducedMotion = false,
  onTogglePause,
  onRetry,
  onCancel,
  onDismiss,
  onOpenMenu,
}: DownloadRowProps) {
  const percent = progressPercent(item);
  const terminal = isTerminal(item.state);
  const live = item.state === "downloading";
  const rate = live ? formatRate(item.speedBytesPerSecond) : null;
  const eta = live ? formatEta(item.etaSeconds) : null;
  const sparkPoints = speedHistory.length >= 2 && live
    ? speedHistoryPoints(speedHistory, SPARK_WIDTH, SPARK_HEIGHT)
    : null;
  const sparkFirst = sparkPoints?.split(" ")[0].split(",").map(Number) ?? null;
  const sparkLast = sparkPoints?.split(" ").at(-1)?.split(",").map(Number) ?? null;
  const kind = fileTypeInfo(item.fileName);

  const sideValue = live
    ? (rate ?? "Measuring…")
    : item.state === "completed"
      ? formatBytes(item.bytesWritten)
      : percent !== null && percent > 0
        ? `${Math.floor(percent)}%`
        : "—";

  const sideSub = live
    ? (eta ? null : "Transfer running")
    : item.state === "completed"
      ? "Saved to disk"
      : item.state === "paused"
        ? "Paused by you"
        : item.state === "preparing"
          ? "Opening connection"
          : item.state === "finalizing"
            ? "Syncing to disk"
            : item.state === "retrying"
              ? "Automatic retry"
              : item.state === "pausing"
                ? "Pausing…"
                : item.state === "cancelling"
                  ? "Cancelling…"
                  : null;

  const meta = item.state === "retrying"
    ? "Retry scheduled"
    : item.state === "scheduled"
      ? item.scheduledAtUnixMs ? `Scheduled for ${new Date(item.scheduledAtUnixMs).toLocaleString()}` : "Scheduled"
      : item.state === "queued"
      ? "Waiting for a transfer slot"
      : item.state === "completed"
        ? `${formatBytes(item.bytesWritten)} saved`
        : percent !== null
          ? `${formatBytes(item.bytesWritten)} of ${formatBytes(item.totalBytes ?? 0)}`
          : item.totalBytes === null
            ? `${formatBytes(item.bytesWritten)} · size unknown`
            : formatBytes(item.bytesWritten);

  return (
    <article
      className={`download-row state-${item.state}`}
      onContextMenu={(event) => {
        event.preventDefault();
        onOpenMenu(item, event);
      }}
    >
      {/* File-type tile: the extension lettering doubles as the thumbnail. */}
      <div className={`row-icon tone-${kind.tone}`} aria-hidden="true">
        {kind.tone === "generic" ? <Icon name="file" size={19} /> : <span className="row-icon-code">{kind.code}</span>}
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
          <div className="row-progressline">
            <ProgressTrack
              value={percent ?? (item.state === "completed" ? 100 : null)}
              completed={item.state === "completed"}
              indeterminateWidth={item.state === "queued" || item.state === "scheduled" ? 0 : 6}
              className="row-progress"
              ariaLabel={`${item.fileName} progress`}
            />
            <span className="row-percent">
              {live ? (percent !== null ? `${Math.floor(percent)}%` : null) : item.state === "completed" ? "100%" : null}
            </span>
          </div>
        )}
        <div className="row-meta">
          {item.state === "queued" && (
            <AILoader variant="dots" reduced={reducedMotion} className="row-queue-indicator" />
          )}
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
          {item.state === "finalizing" && <span className="row-meta-dim">Syncing to disk…</span>}
        </div>
      </div>

      <div className="row-side">
        <span className={`row-side-value${live ? " row-rate" : ""}`}>{sideValue}</span>
        {sparkPoints && (
          <svg
            className="row-spark"
            viewBox={`0 0 ${SPARK_WIDTH} ${SPARK_HEIGHT}`}
            preserveAspectRatio="none"
            role="img"
            aria-label={`Recent speed ${rate ?? "measuring"}`}
          >
            <polygon className="row-spark-area" points={`${sparkPoints} ${SPARK_WIDTH},${SPARK_HEIGHT} 0,${SPARK_HEIGHT}`} />
            <polyline className="row-spark-line" points={sparkPoints} />
            {sparkFirst && <circle className="row-spark-dot" cx={sparkFirst[0]} cy={sparkFirst[1]} r="1.6" />}
            {sparkLast && <circle className="row-spark-dot current" cx={sparkLast[0]} cy={sparkLast[1]} r="1.9" />}
          </svg>
        )}
        {eta && <span className="row-eta">{eta}</span>}
        {sideSub && <span className="row-side-sub">{sideSub}</span>}
        <div className="row-actions">
          {(item.state === "downloading" || item.state === "preparing" || item.state === "paused") && (
            <Tooltip label={item.state === "paused" ? "Resume download" : "Pause download"}>
              <button
                type="button"
                className="row-action"
                onClick={() => onTogglePause(item)}
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
              onClick={() => onCancel(item)}
              disabled={pending}
              aria-label={`Cancel ${item.fileName}`}
            >
              {pending ? <Spinner /> : <Icon name="close" size={16} />}
            </button>
          )}
          {(item.state === "failed" || item.state === "cancelled" || item.state === "interrupted") && (
            <button type="button" className="row-action" onClick={() => onRetry(item)} disabled={pending} aria-label={`Retry ${item.fileName}`}>
              <Icon name="refresh" size={16} />
            </button>
          )}
          {terminal && (
            <button type="button" className="row-action" onClick={() => onDismiss(item)} disabled={pending} aria-label={`Remove ${item.fileName} from the list`}>
              {pending ? <Spinner /> : <Icon name="trash" size={16} />}
            </button>
          )}
          <Tooltip label="More actions">
            <button
              type="button"
              className="row-action more"
              onClick={(event) => onOpenMenu(item, event)}
              aria-label={`More actions for ${item.fileName}`}
            >
              <Icon name="more" size={16} />
            </button>
          </Tooltip>
        </div>
        <span className="sr-only">{STATE_LABELS[item.state]}</span>
      </div>
    </article>
  );
});
