import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { cn } from "../../lib/utils";

export type ToastKind = "info" | "success" | "error";

export type ToastInput = {
  title: string;
  description?: string;
  kind?: ToastKind;
  /** When true the toast never auto-dismisses (e.g. recovery notices). */
  persistent?: boolean;
};

type ToastRecord = ToastInput & { id: number };

type ToastContextValue = {
  push: (toast: ToastInput) => void;
};

const ToastContext = createContext<ToastContextValue | null>(null);

const AUTO_DISMISS_MS: Record<ToastKind, number> = { info: 6_000, success: 6_000, error: 10_000 };
const MAX_VISIBLE_TOASTS = 4;

let nextToastId = 0;

export function useToast(): ToastContextValue {
  const context = useContext(ToastContext);
  if (!context) throw new Error("useToast must be used inside a ToastProvider");
  return context;
}

/**
 * Fixed-layout toast system (no portal needed): a context provider plus a
 * viewport anchored to the top-right of the main area, below the topbar.
 * The viewport is a polite live region; error toasts additionally carry
 * `role="alert"`. At most 4 toasts are visible — pushing a fifth drops the
 * oldest. Entrance is a subtle CSS slide/fade that the global reduced-motion
 * rules neutralise. Z-index sits above the dialog scrim so command errors
 * remain readable while the add-download dialog is open.
 */
export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<ToastRecord[]>([]);

  const push = useCallback((toast: ToastInput) => {
    const id = ++nextToastId;
    setToasts((current) => [...current, { ...toast, id }].slice(-MAX_VISIBLE_TOASTS));
  }, []);

  const dismiss = useCallback((id: number) => {
    setToasts((current) => current.filter((toast) => toast.id !== id));
  }, []);

  const value = useMemo(() => ({ push }), [push]);

  return (
    <ToastContext.Provider value={value}>
      {children}
      <div className="toast-viewport" aria-live="polite">
        {toasts.map((toast) => (
          <ToastCard key={toast.id} toast={toast} dismiss={dismiss} />
        ))}
      </div>
    </ToastContext.Provider>
  );
}

function ToastCard({ toast, dismiss }: { toast: ToastRecord; dismiss: (id: number) => void }) {
  const kind = toast.kind ?? "info";
  const isError = kind === "error";

  useEffect(() => {
    if (toast.persistent) return;
    const timer = window.setTimeout(() => dismiss(toast.id), AUTO_DISMISS_MS[kind]);
    return () => window.clearTimeout(timer);
  }, [dismiss, toast.id, toast.persistent, kind]);

  return (
    <div className={cn("toast-card", kind)} role={isError ? "alert" : undefined}>
      <span className="toast-dot" aria-hidden="true" />
      <div className="toast-copy">
        <p className="toast-title">{toast.title}</p>
        {toast.description && <p className="toast-description">{toast.description}</p>}
      </div>
      <button type="button" className="toast-close" aria-label="Dismiss notification" onClick={() => dismiss(toast.id)}>
        <svg aria-hidden="true" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round">
          <path d="m6 6 12 12M18 6 6 18" />
        </svg>
      </button>
    </div>
  );
}
