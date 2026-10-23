import { useEffect, useLayoutEffect, useRef, useState } from "react";

export type MenuItemSpec = {
  id: string;
  label: string;
  icon?: React.ReactNode;
  danger?: boolean;
  disabled?: boolean;
  onSelect: () => void;
};

type ContextMenuProps = {
  x: number;
  y: number;
  items: MenuItemSpec[];
  onClose: () => void;
};

/**
 * Dependency-free right-click menu (WAI-ARIA menu pattern). Positions itself
 * inside the viewport, closes on outside press / Escape / re-right-click, and
 * supports full arrow-key navigation with Home/End. Focus moves to the first
 * enabled item on open; Tab closes like a native menu.
 */
export function ContextMenu({ x, y, items, onClose }: ContextMenuProps) {
  const panelRef = useRef<HTMLDivElement | null>(null);
  const [position, setPosition] = useState({ x, y });

  useLayoutEffect(() => {
    const panel = panelRef.current;
    if (!panel) return;
    const rect = panel.getBoundingClientRect();
    const width = rect.width || 200;
    const height = rect.height || items.length * 34 + 12;
    setPosition({
      x: Math.max(8, Math.min(x, window.innerWidth - width - 8)),
      y: Math.max(8, Math.min(y, window.innerHeight - height - 8)),
    });
    panel.querySelector<HTMLElement>("button:not(:disabled)")?.focus();
  }, [x, y, items.length]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [onClose]);

  function handlePanelKeyDown(event: React.KeyboardEvent<HTMLDivElement>) {
    const focusables = Array.from(
      panelRef.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? [],
    );
    if (!focusables.length) return;
    const currentIndex = focusables.findIndex((element) => element === document.activeElement);
    let nextIndex: number | null = null;
    if (event.key === "ArrowDown") nextIndex = (currentIndex + 1 + focusables.length) % focusables.length;
    if (event.key === "ArrowUp") nextIndex = (currentIndex - 1 + focusables.length) % focusables.length;
    if (event.key === "Home") nextIndex = 0;
    if (event.key === "End") nextIndex = focusables.length - 1;
    if (event.key === "Tab") {
      event.preventDefault();
      onClose();
      return;
    }
    if (nextIndex !== null) {
      event.preventDefault();
      focusables[nextIndex]?.focus();
    }
  }

  return (
    <div
      className="menu-scrim"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
      onContextMenu={(event) => {
        event.preventDefault();
        onClose();
      }}
    >
      <div
        ref={panelRef}
        role="menu"
        aria-orientation="vertical"
        className="context-menu"
        style={{ left: position.x, top: position.y }}
        onKeyDown={handlePanelKeyDown}
      >
        {items.map((item) => (
          <button
            key={item.id}
            type="button"
            role="menuitem"
            disabled={item.disabled}
            className={`menu-item${item.danger ? " danger" : ""}`}
            onClick={() => {
              onClose();
              item.onSelect();
            }}
          >
            {item.icon && <span className="menu-item-icon">{item.icon}</span>}
            <span>{item.label}</span>
          </button>
        ))}
      </div>
    </div>
  );
}
