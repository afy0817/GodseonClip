// npm run test:ts — the dictionaries: the same keys in both, no Korean left in English, and no
// user-facing Korean left in the code outside `src/lib/i18n/` (comments aside).
import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import type { Msg } from "../ipc";
import { en } from "./en.ts";
import { ko } from "./ko.ts";
import { plural, say } from "./text.ts";

const HANGUL = /[ㄱ-ㆎ가-힣]/;

/** "a.b.c" for every leaf (a string, a function, an array item). */
function leaves(o: unknown, at = ""): Map<string, unknown> {
  const out = new Map<string, unknown>();
  if (o !== null && typeof o === "object") {
    for (const [k, v] of Object.entries(o)) for (const [p, x] of leaves(v, at ? `${at}.${k}` : k)) out.set(p, x);
  } else out.set(at, o);
  return out;
}

test("both dictionaries have the same keys, of the same kind", () => {
  const k = leaves(ko);
  const e = leaves(en);
  assert.deepEqual([...e.keys()].sort(), [...k.keys()].sort());
  for (const [path, v] of k) assert.equal(typeof e.get(path), typeof v, path);
});

test("the English dictionary has no Korean", () => {
  // Functions are called with sample arguments; those that need other shapes are skipped.
  const samples = [2, "x", "y", "z"];
  for (const [path, v] of leaves(en)) {
    if (path.startsWith("msg.")) continue;
    let text: unknown = v;
    if (typeof v === "function") {
      try {
        text = (v as (...a: unknown[]) => unknown)(...samples.slice(0, v.length));
      } catch {
        continue;
      }
    }
    if (typeof text === "string") assert.doesNotMatch(text, HANGUL, path);
  }
});

test("Rust's codes in both languages, nested ones too", () => {
  const m: Msg = { code: "enteredPathUnusable", why: { code: "ffmpegTooOld", version: "5.1.2", min: "6.0" } };
  assert.equal(say(ko, m), "입력한 경로는 쓸 수 없어 저장하지 않았습니다. FFmpeg 5.1.2은(는) 최소 버전 6.0보다 낮습니다.");
  assert.equal(say(en, m), "The path you entered can't be used, so it wasn't saved. FFmpeg 5.1.2 is older than the minimum version, 6.0.");
  assert.equal(say(en, { code: "raw", text: "moov atom not found" }), "moov atom not found");
  assert.equal(say(en, { code: "fileNotFound", path: null }), "File not found.");
  assert.equal(say(ko, { code: "suffixBadChar", ch: null }), "이름 뒤에 붙일 말에 제어 문자는 쓸 수 없습니다.");
  assert.equal(say(en, { code: "openGopJoin", at: 2 }).includes("2.000s"), true);
  for (const code of Object.keys(en.msg)) assert.ok(code in ko.msg, code);
  assert.deepEqual([plural(1, "file"), plural(3, "file"), plural(2, "box", "boxes")], ["1 file", "3 files", "2 boxes"]);
});

/** Source files of the app, the dictionaries and tests left out. */
function sources(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return path.endsWith(join("lib", "i18n")) ? [] : sources(path);
    return /\.tsx?$/.test(name) && !name.endsWith(".test.ts") ? [path] : [];
  });
}

/** The code without comments (`//` to the end of a line, `/* … *\/`, JSX `{/* … *\/}`). */
function code(text: string): string {
  return text.replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|[^:"'`\\])\/\/.*$/gm, "$1");
}

test("no Korean left in the code outside the dictionaries", () => {
  // fileURLToPath, not URL.pathname: on Windows the pathname is "/D:/…" and fs reads it as "D:\D:\…".
  const root = fileURLToPath(new URL("../../", import.meta.url));
  const found: string[] = [];
  for (const file of sources(root)) {
    code(readFileSync(file, "utf8"))
      .split("\n")
      .forEach((line, i) => {
        if (HANGUL.test(line)) found.push(`${file.slice(root.length)}:${i + 1}: ${line.trim()}`);
      });
  }
  assert.deepEqual(found, []);
});
