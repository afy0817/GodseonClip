// Editor keyboard shortcuts (canvas mockup Editor.dc.html): I · O, ← → one frame, Shift + ← → one second,
// Space play / pause; in 구간 제거 also A (add the marked range) and Delete / Backspace (remove the
// selected one). Pure functions; EditorScreen listens on the window.

export type Shortcut =
  | "markIn"
  | "markOut"
  | "prevFrame"
  | "nextFrame"
  | "back1s"
  | "forward1s"
  | "togglePlay"
  | "addRange"
  | "deleteRange";

/** Shortcuts that act again while the key is held down. */
export const REPEATING: ReadonlySet<Shortcut> = new Set(["prevFrame", "nextFrame", "back1s", "forward1s"]);

export interface KeyLike {
  key: string;
  code: string;
  shiftKey: boolean;
  altKey: boolean;
  ctrlKey: boolean;
  metaKey: boolean;
}

export function shortcutFor(e: KeyLike): Shortcut | null {
  // ⌘ / Ctrl / Option combinations belong to the app menu and the system.
  if (e.altKey || e.ctrlKey || e.metaKey) return null;
  if (e.key === "ArrowLeft") return e.shiftKey ? "back1s" : "prevFrame";
  if (e.key === "ArrowRight") return e.shiftKey ? "forward1s" : "nextFrame";
  if (e.shiftKey) return null;
  if (e.code === "Space" || e.key === " ") return "togglePlay";
  if (e.key === "Delete" || e.key === "Backspace") return "deleteRange";
  if (isLetter(e, "i")) return "markIn";
  if (isLetter(e, "o")) return "markOut";
  if (isLetter(e, "a")) return "addRange";
  return null;
}

/**
 * The letter typed, or — when the key types no Latin letter, e.g. "ㅑ" with Korean input on —
 * the key in that letter's place.
 */
function isLetter(e: KeyLike, letter: string): boolean {
  const k = e.key.toLowerCase();
  if (/^[a-z]$/.test(k)) return k === letter;
  return e.code === `Key${letter.toUpperCase()}`;
}

export interface TargetLike {
  tagName?: string;
  type?: string;
  isContentEditable?: boolean;
}

const BUTTON_INPUTS = new Set(["button", "submit", "reset", "image", "color", "file"]);

/** The focused element uses this key itself (typing in a field, moving a slider, ticking a box). */
export function keyBelongsToTarget(target: TargetLike | null, s: Shortcut): boolean {
  if (!target) return false;
  if (target.isContentEditable) return true;
  const tag = target.tagName?.toUpperCase();
  if (tag === "TEXTAREA" || tag === "SELECT") return true;
  if (tag !== "INPUT") return false;
  const type = (target.type ?? "text").toLowerCase();
  // A slider (the zoom) takes the arrows only.
  if (type === "range") return REPEATING.has(s);
  if (type === "checkbox" || type === "radio") return s === "togglePlay";
  return !BUTTON_INPUTS.has(type);
}
