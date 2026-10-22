import type { ButtonHTMLAttributes } from "react";

/**
 * Adapted from the shadcn/ui Button pattern (MIT licensed).
 * Kept dependency-free and styled with Velo's Tailwind design tokens.
 */
type ButtonVariant = "primary" | "secondary" | "quiet";

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: ButtonVariant;
};

const variants: Record<ButtonVariant, string> = {
  primary: "border-action/75 bg-action text-action-ink shadow-sm hover:bg-action/90 hover:border-action",
  secondary: "border-border-subtle bg-surface text-foreground hover:border-action/45 hover:bg-background",
  quiet: "border-transparent bg-transparent text-muted-foreground hover:bg-surface hover:text-foreground",
};

export function Button({ variant = "secondary", className = "", type = "button", ...props }: ButtonProps) {
  return (
    <button
      type={type}
      className={`inline-flex min-h-[38px] items-center justify-center gap-2 rounded-control border px-3.5 font-sans text-xs font-semibold transition duration-150 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus-ring disabled:cursor-not-allowed disabled:opacity-55 ${variants[variant]} ${className}`}
      {...props}
    />
  );
}
