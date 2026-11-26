// What the header's notification center shows: the queue's running and waiting items, the ones that
// finished last, and which finished ones the user has not seen yet. Pure functions, no IPC
// (`npm run test:ts` runs them: imports are types or `.ts` modules).

import type { QueueItem } from "../../lib/ipc";
import { groupItems } from "../queue/queueView.ts";

/** Finished items listed at most (the rest are on the queue screen). */
export const RECENT_LIMIT = 5;

export interface NotifyGroups {
  running: QueueItem[];
  waiting: QueueItem[];
  /** The last finished first, at most RECENT_LIMIT. */
  recent: QueueItem[];
}

export function notifyGroups(items: QueueItem[]): NotifyGroups {
  const g = groupItems(items);
  return { running: g.running, waiting: g.waiting, recent: g.finished.slice(0, RECENT_LIMIT) };
}

/** The highest `finishedOrder` among the items: what "seen" moves to when the panel opens. */
export function lastFinishedOrder(items: QueueItem[]): number {
  return items.reduce((m, i) => Math.max(m, i.finishedOrder ?? 0), 0);
}

export interface Unseen {
  /** Finished since the panel was last opened. */
  count: number;
  /** One of them failed (the mark turns orange). */
  failed: boolean;
}

/** Items that finished after `seenOrder` (the last `finishedOrder` the user saw). */
export function unseenOf(items: QueueItem[], seenOrder: number): Unseen {
  const fresh = items.filter((i) => (i.finishedOrder ?? 0) > seenOrder);
  return { count: fresh.length, failed: fresh.some((i) => i.state === "failed") };
}

/**
 * How far the running items are together, 0–100 (their percents averaged); null when nothing runs.
 * Drawn as the bar along the bottom of the header's bell.
 */
export function overallPercent(items: QueueItem[]): number | null {
  const running = items.filter((i) => i.state === "running");
  if (running.length === 0) return null;
  return running.reduce((s, i) => s + (i.progress?.percent ?? 0), 0) / running.length;
}
