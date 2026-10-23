import { cn } from "../../lib/utils";

type ProgressTrackProps = {
  /** Percentage 0-100, or `null` when the total size is unknown. */
  value: number | null;
  /** Green "done" variant. */
  completed?: boolean;
  /**
   * Decorative fill width (percent) used while the value is unknown,
   * matching the old inline card behaviour (0 for queued, 4 while moving).
   */
  indeterminateWidth?: number;
  className?: string;
  ariaLabel?: string;
};

/**
 * The shared `.progress-track` bar. Determinate values expose
 * `role="progressbar"` with valuemix/min/max; unknown sizes render a purely
 * decorative, aria-hidden track so screen readers are never told a fake
 * percentage.
 */
export function ProgressTrack({ value, completed = false, indeterminateWidth = 4, className = "", ariaLabel }: ProgressTrackProps) {
  if (value === null) {
    return (
      <div aria-hidden="true" className={cn("progress-track", completed && "complete", className)}>
        <span style={{ width: `${indeterminateWidth}%` }} />
      </div>
    );
  }
  const clamped = Math.min(100, Math.max(0, value));
  return (
    <div
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(clamped)}
      aria-label={ariaLabel}
      className={cn("progress-track", completed && "complete", className)}
    >
      <span style={{ width: `${clamped}%` }} />
    </div>
  );
}
