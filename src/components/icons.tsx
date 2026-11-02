import type { ReactNode } from "react";

export type IconName =
  | "download"
  | "queue"
  | "check"
  | "settings"
  | "plus"
  | "search"
  | "sun"
  | "moon"
  | "folder"
  | "close"
  | "globe"
  | "arrow"
  | "pause"
  | "alert"
  | "clock"
  | "cancel"
  | "info"
  | "refresh"
  | "trash"
  | "shield"
  | "drive"
  | "spark"
  | "calendar"
  | "more"
  | "eye"
  | "file"
  | "activity"
  | "bell"
  | "minimize";

const PATHS: Record<IconName, ReactNode> = {
  download: <><path d="M12 3v11" /><path d="m7 10 5 5 5-5" /><path d="M5 20h14" /></>,
  queue: <><path d="M4 6h12M4 12h12M4 18h7" /><path d="m16 15 3 3-3 3" /><path d="M20 18h-6" /></>,
  check: <><circle cx="12" cy="12" r="9" /><path d="m8 12 2.5 2.5L16 9" /></>,
  settings: <><circle cx="12" cy="12" r="3" /><path d="m19.4 15 .1.1a1.7 1.7 0 0 1-2.4 2.4l-.1-.1a1.7 1.7 0 0 0-2.9 1.2v.2a1.7 1.7 0 0 1-3.4 0v-.2A1.7 1.7 0 0 0 8 17.4l-.1.1a1.7 1.7 0 0 1-2.4-2.4l.1-.1a1.7 1.7 0 0 0-1.2-2.9H4.2a1.7 1.7 0 0 1 0-3.4h.2A1.7 1.7 0 0 0 5.6 6.8l-.1-.1a1.7 1.7 0 0 1 2.4-2.4l.1.1a1.7 1.7 0 0 0 2.9-1.2V3a1.7 1.7 0 0 1 3.4 0v.2a1.7 1.7 0 0 0 2.9 1.2l.1-.1a1.7 1.7 0 0 1 2.4 2.4l-.1.1a1.7 1.7 0 0 0 1.2 2.9h.2a1.7 1.7 0 0 1 0 3.4h-.2a1.7 1.7 0 0 0-1.3 1.9Z" /></>,
  plus: <><path d="M12 5v14M5 12h14" /></>,
  search: <><circle cx="10.8" cy="10.8" r="6.8" /><path d="m16 16 4 4" /></>,
  sun: <><circle cx="12" cy="12" r="4" /><path d="M12 2v2m0 16v2M4.9 4.9l1.4 1.4m11.4 11.4 1.4 1.4M2 12h2m16 0h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" /></>,
  moon: <path d="M20.5 14.2A8.5 8.5 0 0 1 9.8 3.5 8.8 8.8 0 1 0 20.5 14.2Z" />,
  folder: <><path d="M3 6.5A1.5 1.5 0 0 1 4.5 5H10l2 2h7.5A1.5 1.5 0 0 1 21 8.5v9a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17.5Z" /><path d="M3 9h18" /></>,
  close: <><path d="m6 6 12 12M18 6 6 18" /></>,
  globe: <><circle cx="12" cy="12" r="9" /><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18" /></>,
  arrow: <><path d="M5 12h14M13 6l6 6-6 6" /></>,
  pause: <><path d="M9 5v14M15 5v14" /></>,
  alert: <><path d="M10.3 4.3 2.7 17.5A1.7 1.7 0 0 0 4.2 20h15.6a1.7 1.7 0 0 0 1.5-2.5L13.7 4.3a2 2 0 0 0-3.4 0Z" /><path d="M12 9v4m0 3h.01" /></>,
  clock: <><circle cx="12" cy="12" r="9" /><path d="M12 7v5l3 2" /></>,
  cancel: <><circle cx="12" cy="12" r="9" /><path d="m9 9 6 6m0-6-6 6" /></>,
  info: <><circle cx="12" cy="12" r="9" /><path d="M12 11v5m0-8h.01" /></>,
  refresh: <><path d="M20 12a8 8 0 1 1-2.34-5.66" /><path d="M20 4v4h-4" /></>,
  trash: <><path d="M4 7h16" /><path d="M9 7V5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2" /><path d="M6.5 7 7.4 19a2 2 0 0 0 2 1.9h5.2a2 2 0 0 0 2-1.9L17.5 7" /></>,
  shield: <><path d="M12 3 5 5.8v5.4c0 4.3 2.9 8.2 7 9.3 4.1-1.1 7-5 7-9.3V5.8Z" /><path d="m9.2 12 2 2 3.6-3.8" /></>,
  drive: <><rect x="3" y="4" width="18" height="7" rx="1.5" /><rect x="3" y="13" width="18" height="7" rx="1.5" /><path d="M7 7.5h.01M7 16.5h.01" /></>,
  spark: <><path d="m12 3 1.6 6.4L20 11l-6.4 1.6L12 19l-1.6-6.4L4 11l6.4-1.6Z" /><path d="m19 15 .8 2.2L22 18l-2.2.8L19 21l-.8-2.2L16 18l2.2-.8Z" /></>,
  calendar: <><rect x="3.5" y="5" width="17" height="16" rx="2.5" /><path d="M8 3v4M16 3v4M3.5 10.5h17" /></>,
  more: <><circle cx="5" cy="12" r="1.15" fill="currentColor" stroke="none" /><circle cx="12" cy="12" r="1.15" fill="currentColor" stroke="none" /><circle cx="19" cy="12" r="1.15" fill="currentColor" stroke="none" /></>,
  eye: <><path d="M2.6 12S6.2 5.8 12 5.8 21.4 12 21.4 12 17.8 18.2 12 18.2 2.6 12 2.6 12Z" /><circle cx="12" cy="12" r="2.9" /></>,
  file: <><path d="M13.5 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8.5Z" /><path d="M13.5 3v5.5H19" /></>,
  activity: <path d="M3 12h3.6L9 6.4l4.6 11.2 2.4-5.6H21" />,
  bell: <><path d="M3 7v3a5 5 0 0 0 10 0V7M3 7v3a5 5 0 0 1 10 0V7" /><path d="M7 15h10v2H7z" /></>,
  minimize: <path d="M1 5.5h8" />,
};

export function Icon({ name, size = 18 }: { name: IconName; size?: number }) {
  return (
    <svg
      aria-hidden="true"
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      {PATHS[name]}
    </svg>
  );
}
