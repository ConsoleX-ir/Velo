import {
  cloneElement,
  useEffect,
  useId,
  useRef,
  useState,
  type FocusEvent,
  type HTMLAttributes,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type ReactElement,
  type ReactNode,
} from "react";

type TooltipProps = {
  /** Tooltip copy. Rendered inside the `[role="tooltip"]` bubble. */
  label: ReactNode;
  /** Single interactive element (usually an icon-only button) that gets the trigger behaviour. */
  children: ReactElement<HTMLAttributes<HTMLElement>>;
  /** Delay before the bubble appears, for hover and keyboard focus alike. */
  delayMs?: number;
};

/**
 * Lightweight, dependency-free tooltip following the WAI-ARIA tooltip
 * pattern (and the composition approach used by shadcn/ui's Tooltip):
 * the trigger keeps all of its own props (notably `aria-label`) and is
 * linked to the bubble via `aria-describedby` only while the bubble is
 * open. Shows on hover AND keyboard focus after a short delay; hides on
 * blur, pointer leave, Escape, and any scroll. The bubble is absolutely
 * positioned above the trigger inside a relative wrapper, so no portal or
 * measuring library is needed. Pure CSS transitions mean the global
 * reduced-motion rules in styles.css neutralise it automatically.
 */
export function Tooltip({ label, children, delayMs = 300 }: TooltipProps) {
  const [open, setOpen] = useState(false);
  const timerRef = useRef<number | null>(null);
  const tooltipId = useId();

  useEffect(() => () => {
    if (timerRef.current !== null) window.clearTimeout(timerRef.current);
  }, []);

  useEffect(() => {
    if (!open) return;
    const hide = () => setOpen(false);
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") hide();
    };
    // Capture phase: hide as soon as any scrollable region moves.
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("scroll", hide, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("scroll", hide, true);
    };
  }, [open]);

  function scheduleOpen() {
    if (timerRef.current !== null) window.clearTimeout(timerRef.current);
    timerRef.current = window.setTimeout(() => {
      timerRef.current = null;
      setOpen(true);
    }, delayMs);
  }

  function cancel() {
    if (timerRef.current !== null) {
      window.clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    setOpen(false);
  }

  const childProps = children.props;

  return (
    <span className="relative inline-flex">
      {cloneElement(children, {
        "aria-describedby": open ? tooltipId : childProps["aria-describedby"],
        onMouseEnter: (event: ReactMouseEvent<HTMLElement>) => {
          childProps.onMouseEnter?.(event);
          scheduleOpen();
        },
        onMouseLeave: (event: ReactMouseEvent<HTMLElement>) => {
          childProps.onMouseLeave?.(event);
          cancel();
        },
        onFocus: (event: FocusEvent<HTMLElement>) => {
          childProps.onFocus?.(event);
          scheduleOpen();
        },
        onBlur: (event: FocusEvent<HTMLElement>) => {
          childProps.onBlur?.(event);
          cancel();
        },
        onKeyDown: (event: ReactKeyboardEvent<HTMLElement>) => {
          childProps.onKeyDown?.(event);
          if (event.key === "Escape") cancel();
        },
      })}
      {open && (
        <span id={tooltipId} role="tooltip" className="tooltip-bubble">
          {label}
        </span>
      )}
    </span>
  );
}
