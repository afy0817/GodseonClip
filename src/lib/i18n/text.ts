// Wording helpers shared by the dictionaries and the screens. Pure (`npm run test:ts` runs them):
// imports are types only.

import type { Lang, Msg, MsgCode, MsgOf } from "../ipc";
import type { Messages } from "./ko.ts";

/** Each language by its own name (the language picker shows them the same in every language). */
export const LANGUAGE_NAMES: Record<Lang, string> = { ko: "한국어", en: "English" };

/** The picker's heading in both languages, so it can be found in either. */
export const LANGUAGE_HEADING = "언어 · Language";

/** One Rust code's sentence; `say` words a nested Msg (`why`) the same way. */
export type MsgTexts = { [C in MsgCode]: (m: MsgOf<C>, say: (m: Msg) => string) => string };

/** A Msg in the dictionary's language. */
export function say(t: Messages, m: Msg): string {
  const word = t.msg[m.code] as (m: Msg, say: (m: Msg) => string) => string;
  return word(m, (inner) => say(t, inner));
}

/** English count: "1 file" · "3 files" (`many` when the plural is not just +s). */
export function plural(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}
