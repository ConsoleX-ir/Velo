import { useEffect, useRef } from "react";

/*
  AuroraBackground — Velo's lightweight replacement for the removed WebGL
  ShaderBackground canvas.

  Composition (all CSS, no canvas, no animation loop):
    .aurora-bg          static aurora field: fixed radial gradients in the
                        "Aurora Stream" turquoise/violet duotone, themed via
                        custom properties. Painted once, then idle.
    .aurora-spot        the pointer spotlight, mounted only when motion is
                        allowed (`interactive`). A single fixed-size radial
                        gradient moved with `transform: translate3d(...)`
                        from two CSS custom properties, so pointer updates
                        cost one composited transform — no layout, no paint
                        of the app behind it.

  Pointer handling: one shared `pointermove` listener on `window` writes
  `--spot-x` / `--spot-y` straight to the spotlight element's style — no
  React state, so pointer movement never re-renders the app. Updates are
  coalesced to at most one write per animation frame and only while the
  pointer is actually moving; nothing runs when the pointer rests, leaves
  the window, the app loses focus, or the OS asks for reduced motion.

  Smoothness comes from a CSS `transition` on the transform: retargeting it
  on each coalesced update produces the soft trailing follow, and the
  transition simply finishes when the pointer settles — no rAF loop needed.
*/

export function AuroraBackground({ interactive = false }: { interactive?: boolean }) {
  const spotRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const spot = spotRef.current;
    if (!interactive || !spot) return;

    // jsdom (and some webviews) do not implement matchMedia; degrade to
    // "no OS-level reduced-motion request" rather than crash.
    const reducedQuery: Pick<MediaQueryList, "matches" | "addEventListener" | "removeEventListener"> =
      typeof window.matchMedia === "function"
        ? window.matchMedia("(prefers-reduced-motion: reduce)")
        : { matches: false, addEventListener: () => undefined, removeEventListener: () => undefined };

    let frame = 0;
    let nextX = 0;
    let nextY = 0;
    let shown = false;
    let focused = document.hasFocus();

    const flush = () => {
      frame = 0;
      spot.style.setProperty("--spot-x", `${nextX}px`);
      spot.style.setProperty("--spot-y", `${nextY}px`);
    };

    const show = () => {
      if (!shown) {
        shown = true;
        spot.dataset.shown = "true";
      }
    };

    const hide = () => {
      if (frame) {
        cancelAnimationFrame(frame);
        frame = 0;
      }
      if (shown) {
        shown = false;
        delete spot.dataset.shown;
      }
    };

    const onPointerMove = (event: PointerEvent) => {
      // Idle in every inactive case: unfocused window, or the OS prefers
      // reduced motion (the CSS layer is hidden in that case anyway).
      if (!focused || reducedQuery.matches) return;
      nextX = event.clientX;
      nextY = event.clientY;
      if (!shown) {
        // First sighting: snap the spotlight under the pointer (transform
        // transition disabled) while the opacity fade still runs, so it
        // appears in place instead of sliding over from a stale position.
        spot.classList.add("aurora-spot-snap");
        flush();
        show();
        // Commit the snapped style, then re-enable the trailing transform
        // transition for subsequent movement.
        void getComputedStyle(spot).transform;
        spot.classList.remove("aurora-spot-snap");
        return;
      }
      // Coalesce: at most one style write per frame, scheduled only when a
      // move actually arrived. This is not a render loop — it stops by
      // itself as soon as the pointer stops.
      if (frame === 0) frame = requestAnimationFrame(flush);
    };

    const onReducedChange = () => {
      if (reducedQuery.matches) hide();
    };

    const onFocus = () => {
      focused = document.hasFocus();
    };
    const onBlur = () => {
      focused = false;
      hide();
    };

    window.addEventListener("pointermove", onPointerMove, { passive: true });
    window.addEventListener("blur", onBlur);
    window.addEventListener("focus", onFocus);
    window.addEventListener("pointercancel", hide);
    document.documentElement.addEventListener("pointerleave", hide);
    reducedQuery.addEventListener("change", onReducedChange);

    return () => {
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("blur", onBlur);
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("pointercancel", hide);
      document.documentElement.removeEventListener("pointerleave", hide);
      reducedQuery.removeEventListener("change", onReducedChange);
      if (frame) cancelAnimationFrame(frame);
    };
  }, [interactive]);

  return (
    <div className="aurora-bg" aria-hidden="true">
      {interactive && <div ref={spotRef} className="aurora-spot" />}
    </div>
  );
}
