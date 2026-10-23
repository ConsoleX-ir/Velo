import type { ButtonHTMLAttributes } from "react";
import { cn } from "../../lib/utils";
import { Spinner } from "./spinner";

/**
 * Adapted from the shadcn/ui Button pattern (MIT licensed).
 * Kept dependency-free and styled with Velo's Tailwind design tokens.
 */
type ButtonVariant = "primary" | "secondary" | "quiet";
type ButtonSize = "md" | "sm" | "icon";

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: ButtonVariant;
  size?: ButtonSize;
  /** Shows a spinner and sets `aria-busy` (and disables) while pending. */
  loading?: boolean;
};

const variants: Record<ButtonVariant, string> = {
  primary: "border-action/75 bg-action text-action-ink shadow-sm hover:bg-action/90 hover:border-action",
  secondary: "border-border-subtle bg-surface text-foreground hover:border-action/45 hover:bg-background",
  quiet: "border-transparent bg-transparent text-muted-foreground hover:bg-surface hover:text-foreground",
};

const sizes: Record<ButtonSize, string> = {
  md: "min-h-[38px] px-3.5",
  sm: "min-h-[30px] px-2.5",
  icon: "h-9 w-9 min-h-0 p-0",
};

export function Button({ variant = "secondary", size = "md", loading = false, className = "", type = "button", disabled, children, ...props }: ButtonProps) {
  return (
    <button
      type={type}
      aria-busy={loading || undefined}
      disabled={disabled || loading}
      className={cn(
        "inline-flex items-center justify-center gap-2 rounded-control border font-sans text-xs font-semibold transition duration-150 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus-ring disabled:cursor-not-allowed disabled:opacity-55",
        sizes[size],
        variants[variant],
        className,
      )}
      {...props}
    >
      {loading && <Spinner />}
      {children}
    </button>
  );
}
