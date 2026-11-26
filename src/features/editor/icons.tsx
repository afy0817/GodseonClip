// Stroke icons from the canvas mockup (24×24, currentColor).

import type { ReactNode } from "react";

function Icon({ size = 18, width = 1.8, children }: { size?: number; width?: number; children: ReactNode }) {
  return (
    <svg
      aria-hidden="true"
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={width}
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      {children}
    </svg>
  );
}

export const LogoIcon = () => (
  <Icon size={24}>
    <rect x="3" y="5" width="18" height="14" rx="2" />
    <path d="M9 5v14M15 5v14" />
  </Icon>
);

/** Arrow into a tray: "drop here". */
export const DropIcon = () => (
  <Icon size={32} width={1.6}>
    <path d="M12 3v11M7.5 9.5 12 14l4.5-4.5" />
    <path d="M4 14v4a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-4" />
  </Icon>
);

export const FolderIcon = () => (
  <Icon>
    <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
  </Icon>
);

export const SlidersIcon = () => (
  <Icon>
    <path d="M4 6h9M17 6h3M4 12h3M11 12h9M4 18h11M19 18h1" />
    <circle cx="15" cy="6" r="2" />
    <circle cx="9" cy="12" r="2" />
    <circle cx="17" cy="18" r="2" />
  </Icon>
);

/** The header's notification center. */
export const BellIcon = () => (
  <Icon>
    <path d="M6 9a6 6 0 0 1 12 0c0 6 2.5 8 2.5 8h-17S6 15 6 9" />
    <path d="M10.3 20a1.9 1.9 0 0 0 3.4 0" />
  </Icon>
);

export const QueueIcon = () => (
  <Icon>
    <path d="M9 6h11M9 12h11M9 18h11" />
    <path d="M4 6h1M4 12h1M4 18h1" />
  </Icon>
);

export const BackSecondIcon = () => (
  <Icon>
    <path d="M11 17l-5-5 5-5M18 17l-5-5 5-5" />
  </Icon>
);

export const ForwardSecondIcon = () => (
  <Icon>
    <path d="M13 17l5-5-5-5M6 17l5-5-5-5" />
  </Icon>
);

export const BackFrameIcon = () => (
  <Icon>
    <path d="M6 6v12" />
    <path d="M18 6l-8 6 8 6z" />
  </Icon>
);

export const ForwardFrameIcon = () => (
  <Icon>
    <path d="M18 6v12" />
    <path d="M6 6l8 6-8 6z" />
  </Icon>
);

export const PlayIcon = () => (
  <svg aria-hidden="true" width="20" height="20" viewBox="0 0 24 24">
    <path d="M8 5.5v13l10.5-6.5z" fill="currentColor" />
  </svg>
);

export const PauseIcon = () => (
  <Icon size={20} width={3}>
    <path d="M8 5v14M16 5v14" />
  </Icon>
);

export const InfoIcon = () => (
  <Icon>
    <circle cx="12" cy="12" r="9" />
    <path d="M12 8v5M12 16v.5" />
  </Icon>
);

export const ArrowLeftIcon = () => (
  <Icon>
    <path d="M15 18l-6-6 6-6" />
  </Icon>
);

/** 제거 구간 추가 (canvas mockup). */
export const ScissorsIcon = () => (
  <Icon size={16}>
    <circle cx="6" cy="6" r="3" />
    <circle cx="6" cy="18" r="3" />
    <path d="M8.5 7.5L20 18M8.5 16.5L20 6" />
  </Icon>
);

export const TrashIcon = () => (
  <Icon>
    <path d="M4 7h16" />
    <path d="M9 7V4h6v3" />
    <path d="M6 7l1 13h10l1-13" />
  </Icon>
);

export const PlusIcon = () => (
  <Icon size={16}>
    <path d="M12 5v14M5 12h14" />
  </Icon>
);

export const CloseIcon = () => (
  <Icon size={14}>
    <path d="M6 6l12 12M18 6L6 18" />
  </Icon>
);

/** 작업 큐: move a waiting item up. */
export const ArrowUpIcon = () => (
  <Icon>
    <path d="M12 19V5M6 11l6-6 6 6" />
  </Icon>
);

/** A finished queue item: done. */
export const CheckIcon = () => (
  <Icon width={2}>
    <path d="M5 12l5 5 9-10" />
  </Icon>
);

/** A failed queue item. */
export const WarningIcon = () => (
  <Icon width={2}>
    <path d="M12 4l9 16H3z" />
    <path d="M12 10v4M12 17v.5" />
  </Icon>
);

/** A canceled queue item. */
export const StopIcon = () => (
  <Icon width={2}>
    <circle cx="12" cy="12" r="8" />
    <path d="M6.5 17.5l11-11" />
  </Icon>
);

/** 일괄 변환 drop zone: files up into the list. */
export const UploadIcon = () => (
  <Icon size={22}>
    <path d="M12 15V4M7 9l5-5 5 5" />
    <path d="M4 15v4a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-4" />
  </Icon>
);

export const TurnLeftIcon = () => (
  <Icon size={16}>
    <path d="M4 4v6h6" />
    <path d="M4.5 10A8 8 0 1 1 6 16.5" />
  </Icon>
);

export const TurnRightIcon = () => (
  <Icon size={16}>
    <path d="M20 4v6h-6" />
    <path d="M19.5 10A8 8 0 1 0 18 16.5" />
  </Icon>
);

/** Mirror left to right. */
export const FlipHIcon = () => (
  <Icon size={16}>
    <path d="M12 3v18" />
    <path d="M9 7L4 17h5z" />
    <path d="M15 7l5 10h-5z" />
  </Icon>
);

/** Upside down. */
export const FlipVIcon = () => (
  <Icon size={16}>
    <path d="M3 12h18" />
    <path d="M7 9L17 4v5z" />
    <path d="M7 15l10 5v-5z" />
  </Icon>
);
