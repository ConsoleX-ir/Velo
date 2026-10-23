import { useEffect, useRef, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from "react";
import { cn } from "../../lib/utils";

type DialogProps = {
  open: boolean;
  onClose: () => void;
  /** Allow Escape to close; set to `false` while a submission is busy. */
  closeOnEscape?: boolean;
  /** Allow clicking the scrim to close; set to `false` while busy. */
  closeOnOutsideClick?: boolean;
  labelledBy?: string;
  describedBy?: string;
  panelClassName?: string;
  scrimClassName?: string;
  children: ReactNode;
};

const FOCUSABLE_SELECTOR = 'a[href], button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])';

/**
 * Modal dialog primitive adapted from the shadcn/ui Dialog contract:
 * scrim + `role="dialog" aria-modal="true"`, labelled/described-by
 * passthrough, a Tab focus trap that cycles through the panel's focusable
 * controls, Escape-to-close (gated by `closeOnEscape` for busy states), and
 * focus restoration to the previously focused element when it closes.
 * Rendering is conditional on `open`, so the initial capture of the
 * previously focused element happens during the first render — before React
 * commits `autoFocus` inside the panel.
 */
export function Dialog({ open, onClose, closeOnEscape = true, closeOnOutsideClick = true, labelledBy, describedBy, panelClassName = "", scrimClassName = "", children }: DialogProps) {
  const panelRef = useRef<HTMLDivElement | null>(null);
  const restoreRef = useRef<HTMLElement | null>(null);

  // Capture during render (not in an effect) so it precedes autoFocus commit.
  if (open && restoreRef.current === null && document.activeElement instanceof HTMLElement) {
    restoreRef.current = document.activeElement;
  }

  useEffect(() => {
    if (!open) return;
    // StrictMode remount safety: recapture if the previous cleanup already
    // restored focus, but never steal the captured element from inside the panel.
    if (restoreRef.current === null && document.activeElement instanceof HTMLElement && !panelRef.current?.contains(document.activeElement)) {
      restoreRef.current = document.activeElement;
    }
    return () => {
      const target = restoreRef.current;
      restoreRef.current = null;
      if (target?.isConnected) target.focus();
    };
  }, [open]);

  useEffect(() => {
    if (!open || !closeOnEscape) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [open, closeOnEscape, onClose]);

  if (!open) return null;

  function trapTab(event: ReactKeyboardEvent<HTMLDivElement>) {
    if (event.key !== "Tab") return;
    const controls = event.currentTarget.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR);
    if (!controls.length) return;
    const first = controls[0];
    const last = controls[controls.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  return (
    <div
      className={cn("modal-scrim", scrimClassName)}
      onMouseDown={(event) => {
        if (closeOnOutsideClick && event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={labelledBy}
        aria-describedby={describedBy}
        className={panelClassName}
        onKeyDown={trapTab}
      >
        {children}
      </div>
    </div>
  );
}
