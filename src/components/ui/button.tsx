import type { ButtonHTMLAttributes } from "react";
import { cva } from "class-variance-authority";
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

/**
 * cva variant map for the shadcn-style components Velo integrates (Calendar
 * nav buttons and friends). Variant names follow the shadcn set and are
 * mapped onto the shared Aurora Stream tokens, so composed components stay
 * on-palette without touching the app-level Button below. Kept alongside the
 * existing Button (whose own primary/secondary/quiet API is unchanged).
 */
export const buttonVariants = cva(
  "inline-flex items-center justify-center gap-1.5 whitespace-nowrap rounded-control text-xs font-semibold transition-colors outline-offset-2 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus-ring disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0",
  {
    variants: {
      variant: {
        default: "border border-action/75 bg-action text-action-ink shadow-sm hover:bg-action/90",
        destructive: "border border-transparent bg-destructive text-destructive-foreground shadow-sm hover:bg-destructive/90",
        outline: "border border-input bg-surface shadow-sm hover:bg-accent hover:text-accent-foreground",
        secondary: "border border-border-subtle bg-secondary text-secondary-foreground hover:bg-accent",
        ghost: "text-muted-foreground hover:bg-accent hover:text-accent-foreground",
        link: "text-action underline-offset-4 hover:underline",
        primary: "border border-action/75 bg-action text-action-ink shadow-sm hover:bg-action/90",
        quiet: "text-muted-foreground hover:bg-surface hover:text-foreground",
      },
      size: {
        default: "min-h-[34px] px-3",
        sm: "min-h-[30px] rounded-[7px] px-2.5 text-[11px]",
        lg: "min-h-[38px] px-4",
        icon: "h-9 w-9",
        md: "min-h-[38px] px-3.5",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  },
);

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
