/** 65.4 → "01:05.400", 3723.5 → "1:02:03.500". */
export function formatTime(sec: number): string {
  if (!Number.isFinite(sec) || sec < 0) sec = 0;
  const totalMs = Math.round(sec * 1000);
  const ms = totalMs % 1000;
  const s = Math.floor(totalMs / 1000) % 60;
  const m = Math.floor(totalMs / 60000) % 60;
  const h = Math.floor(totalMs / 3600000);
  const p2 = (n: number) => String(n).padStart(2, "0");
  const tail = `${p2(s)}.${String(ms).padStart(3, "0")}`;
  return h > 0 ? `${h}:${p2(m)}:${tail}` : `${p2(m)}:${tail}`;
}

/** Accepts "65.4", "1:05.4", "01:05.400", "1:02:03". Returns seconds, or null if unreadable. */
export function parseTime(text: string): number | null {
  const t = text.trim();
  if (!/^\d+(:\d{1,2}){0,2}(\.\d+)?$/.test(t)) return null;
  const parts = t.split(":");
  let sec = 0;
  for (const p of parts) sec = sec * 60 + Number(p);
  return Number.isFinite(sec) ? sec : null;
}

export function formatBytes(n: number | null | undefined): string {
  if (n == null) return "—";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = n;
  let i = 0;
  while (v >= 1000 && i < units.length - 1) {
    v /= 1000;
    i++;
  }
  return `${v >= 100 || i === 0 ? Math.round(v) : v.toFixed(1)}${units[i]}`;
}

/** Same rule as plan::keyframe_at_or_before in Rust (1 ms tolerance). */
export function keyframeAtOrBefore(keyframes: number[], t: number): number | null {
  let lo = 0;
  let hi = keyframes.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (keyframes[mid] <= t + 1e-3) lo = mid + 1;
    else hi = mid;
  }
  return lo > 0 ? keyframes[lo - 1] : null;
}
