import { useEffect, useMemo, useRef, useState } from "react";
import { Calendar } from "./calendar";
import { ScrollArea } from "./scroll-area";
import { Icon } from "../icons";

/**
 * Schedule field for Velo's download dialogs: the familiar datetime-local
 * input stays the source of truth (keyboard entry, native min clamping), and
 * a calendar toggle opens the integrated DayPicker calendar plus a half-hour
 * slot list (Radix ScrollArea) that write into the same value.
 *
 * The popover closes on Escape/outside click without dismissing the host
 * dialog: Escape is swallowed in the capture phase before the dialog's
 * window-level listener can see it.
 */

type DateTimeFieldProps = {
  /** id for the underlying input (labelled by the field label). */
  id: string;
  /** Local "YYYY-MM-DDTHH:mm" string, "" when unset. */
  value: string;
  onChange: (next: string) => void;
  /** Minimum selectable instant (epoch ms); earlier slots/dates are disabled. */
  minMs?: number;
  disabled?: boolean;
};

const pad = (part: number) => String(part).padStart(2, "0");

function toLocalInput(ms: number): string {
  const date = new Date(ms);
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function startOfDay(date: Date): Date {
  const copy = new Date(date);
  copy.setHours(0, 0, 0, 0);
  return copy;
}

/** The first half-hour slot that is comfortably in the future. */
function nextSlotTime(): string {
  const candidate = new Date(Date.now() + 10 * 60_000);
  candidate.setSeconds(0, 0);
  if (candidate.getMinutes() <= 0) candidate.setMinutes(0);
  else if (candidate.getMinutes() <= 30) candidate.setMinutes(30);
  else {
    candidate.setMinutes(0);
    candidate.setHours(candidate.getHours() + 1);
  }
  return `${pad(candidate.getHours())}:${pad(candidate.getMinutes())}`;
}

function parseLocal(value: string): Date | null {
  if (!value) return null;
  const ms = new Date(value).getTime();
  return Number.isFinite(ms) ? new Date(ms) : null;
}

const HALF_HOUR_SLOTS = Array.from({ length: 48 }, (_, index) => {
  const hours = Math.floor(index / 2);
  return `${pad(hours)}:${index % 2 === 0 ? "00" : "30"}`;
});

export function DateTimeField({ id, value, onChange, minMs, disabled }: DateTimeFieldProps) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  const selected = parseLocal(value);
  const timeValue = value.includes("T") ? value.split("T")[1] ?? "" : "";
  const minDate = minMs !== undefined ? new Date(minMs) : null;

  // Close on outside pointer-down, and on Escape — swallowed here so the host
  // dialog (window-level Escape listener) stays open.
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (event.target instanceof Node && !rootRef.current?.contains(event.target)) {
        setOpen(false);
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        setOpen(false);
      }
    };
    document.addEventListener("pointerdown", onPointerDown, true);
    document.addEventListener("keydown", onKeyDown, true);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown, true);
      document.removeEventListener("keydown", onKeyDown, true);
    };
  }, [open]);

  /** The day the slot list targets: the picked day, or the nearest allowed day. */
  const slotDay = useMemo(() => {
    if (selected) return selected;
    const now = Date.now();
    return new Date(minMs !== undefined && minMs > now ? minMs : now);
  }, [minMs, selected]);

  const compose = (day: Date, time: string) =>
    `${day.getFullYear()}-${pad(day.getMonth() + 1)}-${pad(day.getDate())}T${time}`;

  const slotDisabled = (slot: string) => {
    if (minMs === undefined) return false;
    const ms = new Date(compose(slotDay, slot)).getTime();
    return !Number.isFinite(ms) || ms < minMs;
  };

  const dayLabel = slotDay.toLocaleDateString(undefined, {
    weekday: "long",
    month: "short",
    day: "numeric",
  });

  return (
    <div className="dt-field" ref={rootRef}>
      <div className="input-wrap">
        <input
          id={id}
          type="datetime-local"
          value={value}
          min={minMs !== undefined ? toLocalInput(minMs) : undefined}
          disabled={disabled}
          onChange={(event) => onChange(event.target.value)}
        />
        <button
          type="button"
          className="icon-button"
          aria-label="Pick a date and time"
          aria-haspopup="dialog"
          aria-expanded={open}
          disabled={disabled}
          onClick={() => setOpen((current) => !current)}
        >
          <Icon name="calendar" size={15} />
        </button>
      </div>
      {open && (
        <div className="dt-popover" role="dialog" aria-label="Choose a start date and time">
          <div className="dt-popover-calendar">
            <Calendar
              mode="single"
              selected={selected ?? undefined}
              defaultMonth={selected ?? minDate ?? undefined}
              onSelect={(day) => {
                if (!day) return;
                onChange(compose(day, timeValue || nextSlotTime()));
              }}
              disabled={minDate ? [{ before: startOfDay(minDate) }] : undefined}
              showOutsideDays={false}
            />
          </div>
          <div className="dt-popover-times">
            <div className="dt-times-label" aria-hidden="true">
              <Icon name="clock" size={12} />
              {dayLabel}
            </div>
            <ScrollArea className="dt-times-scroll">
              <div className="dt-times-grid">
                {HALF_HOUR_SLOTS.map((slot) => (
                  <button
                    key={slot}
                    type="button"
                    className={`dt-slot${timeValue === slot ? " selected" : ""}`}
                    disabled={slotDisabled(slot)}
                    onClick={() => onChange(compose(slotDay, slot))}
                  >
                    {slot}
                  </button>
                ))}
              </div>
            </ScrollArea>
          </div>
        </div>
      )}
    </div>
  );
}
