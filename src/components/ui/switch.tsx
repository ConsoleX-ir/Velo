import type { ButtonHTMLAttributes } from "react";
import { cn } from "../../lib/utils";

type ToggleProps = Omit<ButtonHTMLAttributes<HTMLButtonElement>, "onChange" | "value" | "checked"> & {
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
};

/**
 * Accessible switch (`role="switch"` + `aria-checked`) rendered with Velo's
 * existing `.toggle` / `.toggle.on` styling. Rendered as a real `<button>`,
 * so Space and Enter activate it natively and the global focus-visible
 * outline applies. Mirrors the API shape of shadcn/ui's Switch while staying
 * dependency-free.
 */
export function Toggle({ checked, onCheckedChange, className = "", type = "button", ...props }: ToggleProps) {
  return (
    <button
      type={type}
      role="switch"
      aria-checked={checked}
      className={cn("toggle", checked && "on", className)}
      onClick={() => onCheckedChange(!checked)}
      {...props}
    >
      <span />
    </button>
  );
}
