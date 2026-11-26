// The app's language for React: the dictionary of the language in use (`useT`), and the settings
// screen's pick (`useLanguage`). The language comes from Rust before the first screen (main.tsx), so
// nothing shows in the wrong one first.

import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { api, toMsg, type Lang, type LanguageInfo, type Msg } from "../ipc";
import { en } from "./en";
import { ko, type Messages } from "./ko";

export const DICTIONARIES: Record<Lang, Messages> = { ko, en };

interface LanguageState {
  /** null: the system's language. */
  setting: Lang | null;
  effective: Lang;
  system: Lang;
  /** Why the last pick was not saved (it holds for this run). */
  notice: Msg | null;
  choose: (setting: Lang | null) => void;
}

const LanguageContext = createContext<LanguageState | null>(null);

/** Without Rust (the Vite dev server in a browser): the browser's languages, Korean or English. */
export function browserLanguage(): Lang {
  const tags = navigator.languages?.length ? navigator.languages : [navigator.language];
  for (const tag of tags) {
    const primary = (tag ?? "").toLowerCase().split(/[-_]/)[0];
    if (primary === "ko" || primary === "en") return primary;
  }
  return "en";
}

/** The language to start in: Rust's answer (the saved pick, else the system's). */
export async function startingLanguage(): Promise<LanguageInfo> {
  try {
    return await api.language();
  } catch {
    const system = browserLanguage();
    return { setting: null, effective: system, system, notice: null };
  }
}

export function I18nProvider({ initial, children }: { initial: LanguageInfo; children: ReactNode }) {
  const [info, setInfo] = useState(initial);

  useEffect(() => {
    document.documentElement.lang = info.effective;
  }, [info.effective]);

  const choose = useCallback((setting: Lang | null) => {
    // Switch at once; Rust saves it (and words the native quit dialog in it).
    setInfo((prev) => ({ ...prev, setting, effective: setting ?? prev.system, notice: null }));
    api
      .setLanguage(setting)
      .then(setInfo)
      .catch((e) => setInfo((prev) => ({ ...prev, notice: toMsg(e) })));
  }, []);

  const value = useMemo<LanguageState>(() => ({ ...info, choose }), [info, choose]);
  return <LanguageContext.Provider value={value}>{children}</LanguageContext.Provider>;
}

function useLanguageState(): LanguageState {
  const s = useContext(LanguageContext);
  if (!s) throw new Error("I18nProvider is missing");
  return s;
}

/** The dictionary of the language in use. */
export function useT(): Messages {
  return DICTIONARIES[useLanguageState().effective];
}

/** The settings screen's 언어 · Language. */
export function useLanguage(): LanguageState {
  return useLanguageState();
}
