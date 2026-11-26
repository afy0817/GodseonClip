import { useCallback, useEffect, useRef, useState } from "react";
import { api, toMsg, type MediaInfo, type ThumbnailStrip, type Waveform, type Msg } from "../../lib/ipc";

export interface TimelineSize {
  /** Thumbnail tiles at zoom 1. */
  tiles: number;
  /** Thumbnail height in pixels. */
  height?: number;
  buckets: number;
}

export interface TimelineData {
  /** Strip for the current zoom, or a coarser one (stretched) while that loads. */
  strip: ThumbnailStrip | null;
  wave: Waveform | null;
  thumbError: Msg | null;
  waveError: Msg | null;
  busy: boolean;
}

/** `preview::MAX_THUMBNAILS`. */
const MAX_TILES = 400;
/** Wait for the zoom slider to settle before asking FFmpeg for a denser strip. */
const ZOOM_SETTLE_MS = 300;

const NO_STRIPS: ReadonlyMap<number, ThumbnailStrip> = new Map();
const NOTHING: ReadonlySet<string> = new Set();

interface Opened {
  path: string | null;
  /** Bumped per opened file; results of an older one are dropped. */
  gen: number;
  tiles: number;
  height?: number;
  /** Zoom levels already asked for (each strip is made once per file). */
  asked: Set<number>;
}

/**
 * Thumbnail strip and waveform of the whole file. `size` is read when a file is opened (e.g. from
 * the timeline's width), not on every resize. The waveform is loaded once — ask for enough buckets
 * for the deepest zoom; thumbnails are loaded per whole zoom level (tiles × level) and kept.
 */
export function useTimelineData(info: MediaInfo | null, size: () => TimelineSize, zoom = 1): TimelineData {
  const [strips, setStrips] = useState(NO_STRIPS);
  const [wave, setWave] = useState<Waveform | null>(null);
  const [thumbError, setThumbError] = useState<Msg | null>(null);
  const [waveError, setWaveError] = useState<Msg | null>(null);
  const [loading, setLoading] = useState(NOTHING);
  const sizeRef = useRef(size);
  sizeRef.current = size;
  const opened = useRef<Opened>({ path: null, gen: 0, tiles: 0, asked: new Set() });
  const path = info?.path ?? null;
  const hasVideo = !!info?.video;
  const hasAudio = (info?.audio.length ?? 0) > 0;
  const level = Math.max(1, Math.round(zoom));

  const mark = useCallback((key: string, on: boolean) => {
    setLoading((s) => {
      const next = new Set(s);
      if (on) next.add(key);
      else next.delete(key);
      return next;
    });
  }, []);

  // The UI drops a superseded file's results; FFmpeg still finishes them (known gap).
  const loadStrip = useCallback(
    (lvl: number) => {
      const o = opened.current;
      if (!o.path || o.asked.has(lvl)) return;
      o.asked.add(lvl);
      const { gen } = o;
      const current = () => opened.current.gen === gen;
      const key = `thumbs${lvl}`;
      mark(key, true);
      api
        .thumbnailStrip({ path: o.path, count: Math.min(MAX_TILES, Math.round(o.tiles * lvl)), height: o.height })
        .then((s) => current() && setStrips((m) => new Map(m).set(lvl, s)))
        .catch((e) => current() && setThumbError(toMsg(e)))
        .finally(() => current() && mark(key, false));
    },
    [mark],
  );

  useEffect(() => {
    const gen = opened.current.gen + 1;
    const s = path ? sizeRef.current() : { tiles: 0, buckets: 0, height: undefined };
    opened.current = { path, gen, tiles: s.tiles, height: s.height, asked: new Set() };
    setStrips(NO_STRIPS);
    setWave(null);
    setThumbError(null);
    setWaveError(null);
    setLoading(NOTHING);
    if (!path) return;
    if (hasVideo) loadStrip(1);
    if (hasAudio) {
      const current = () => opened.current.gen === gen;
      mark("wave", true);
      api
        .audioWaveform({ path, buckets: s.buckets })
        .then((w) => current() && setWave(w))
        .catch((e) => current() && setWaveError(toMsg(e)))
        .finally(() => current() && mark("wave", false));
    }
  }, [path, hasVideo, hasAudio, loadStrip, mark]);

  useEffect(() => {
    if (!path || !hasVideo || level <= 1) return;
    const timer = window.setTimeout(() => loadStrip(level), ZOOM_SETTLE_MS);
    return () => window.clearTimeout(timer);
  }, [path, hasVideo, level, loadStrip]);

  let strip: ThumbnailStrip | null = null;
  for (let l = level; l >= 1 && !strip; l--) strip = strips.get(l) ?? null;
  strip ??= strips.values().next().value ?? null;

  return {
    strip,
    wave,
    thumbError,
    waveError,
    busy: loading.has("wave") || loading.has(`thumbs${level}`) || (!strip && loading.size > 0),
  };
}
