import { cn } from "../../lib/utils";

/**
 * Inline busy indicator. Reuses Velo's existing `.button-spinner` CSS
 * (border ring + `spin` keyframes) so busy states stay visually identical
 * across buttons, cards, and loading states. Purely decorative — callers
 * convey the busy condition via `aria-busy` or adjacent copy, and the global
 * reduced-motion rules in styles.css stop the rotation.
 */
export function Spinner({ className = "" }: { className?: string }) {
  return <span aria-hidden="true" className={cn("button-spinner", className)} />;
}
