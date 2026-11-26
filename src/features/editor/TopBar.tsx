// The header of the editor, the 일괄 변환, the 작업 큐 and the settings screens (canvas Editor.dc.html ·
// Convert.dc.html · Queue.dc.html · Setup.dc.html): brand, the work kinds, the queue button with its
// badge, the screen's own actions, settings.

import type { ReactNode } from "react";
import { useT } from "../../lib/i18n/I18n";
import { NotifyButton } from "../notify/Notify";
import type { EditorMode } from "./exportOptions";
import { LogoIcon, QueueIcon, SlidersIcon } from "./icons";

/** The header's work kinds: the editor's two modes and 일괄 변환. */
export type WorkKind = EditorMode | "convert";

interface Props {
  /** The kind shown; null on the queue screen (no kind is pressed there). */
  kind: WorkKind | null;
  onKind: (k: WorkKind) => void;
  /** Running + waiting queue items. */
  queueCount: number;
  /** The queue screen is the one shown. */
  queueCurrent: boolean;
  /** The settings screen is the one shown. */
  settingsCurrent?: boolean;
  onOpenQueue: () => void;
  onOpenSettings: () => void;
  /** Next to the brand: the open file. */
  left?: ReactNode;
  /** Between the queue and settings buttons (파일 열기). */
  children?: ReactNode;
}

export function TopBar({ kind, onKind, queueCount, queueCurrent, settingsCurrent = false, onOpenQueue, onOpenSettings, left, children }: Props) {
  const t = useT();
  const w = t.topBar;
  return (
    <header className="ed-top">
      <div className="ed-top-left">
        <div className="ed-brand">
          <LogoIcon />
          <span>Godseon</span>
        </div>
        {left}
      </div>

      <nav aria-label={w.kinds} className="ed-modes">
        <button type="button" className="ed-mode" aria-pressed={kind === "trim"} onClick={() => onKind("trim")}>
          {t.kind.trim}
        </button>
        <button type="button" className="ed-mode" aria-pressed={kind === "cut"} title={w.cutTitle} onClick={() => onKind("cut")}>
          {t.kind.cut}
        </button>
        <button
          type="button"
          className="ed-mode"
          aria-pressed={kind === "convert"}
          title={w.convertTitle}
          onClick={() => onKind("convert")}
        >
          {t.kind.convert}
        </button>
      </nav>

      <div className="ed-actions">
        <NotifyButton />
        <button
          type="button"
          className="ed-btn ed-queue-btn"
          aria-current={queueCurrent ? "page" : undefined}
          aria-label={queueCount > 0 ? w.queueLabel(queueCount) : w.queue}
          title={w.queueTitle}
          onClick={onOpenQueue}
        >
          <QueueIcon />
          {w.queue}
          {queueCount > 0 && (
            <span className="ed-queue-count" aria-hidden="true">
              {queueCount}
            </span>
          )}
        </button>
        {children}
        <button
          type="button"
          className="ed-btn icon ed-settings-btn"
          aria-label={w.settings}
          aria-current={settingsCurrent ? "page" : undefined}
          title={w.settingsTitle}
          onClick={onOpenSettings}
        >
          <SlidersIcon />
        </button>
      </div>
    </header>
  );
}
