import { type RefObject, useCallback, useEffect, useRef, useState } from "react";
import type { MediaInfo } from "../../lib/ipc";
import { frameDuration, frameStart, stepFrame } from "./format";

/** Seek a hair past the target so float rounding never lands on the previous frame. */
const SEEK_EPS = 0.001;

export interface Player {
  /** Playhead on the original's timeline (the proxy keeps the original's frame times). */
  now: number;
  playing: boolean;
  seek: (t: number) => void;
  /** Pause and move by whole frames. */
  stepFrames: (n: number) => void;
  /** Move by seconds, keeping play / pause. */
  jump: (sec: number) => void;
  toggle: () => void;
  pause: () => void;
  /** Call from the element's loadedmetadata: a new source (e.g. the proxy) resumes at the playhead. */
  restore: () => void;
  /** Spread onto the <video>. */
  events: {
    onPlay: () => void;
    onPause: () => void;
    onEnded: () => void;
    onTimeUpdate: () => void;
    onSeeked: () => void;
  };
}

/** Transport for the editor's <video>. `src` is what the element plays (original or proxy). */
export function usePlayer(ref: RefObject<HTMLVideoElement | null>, info: MediaInfo | null, src: string): Player {
  const v = info?.video ?? null;
  const duration = info?.duration ?? 0;
  const [now, setNowState] = useState(0);
  const [playing, setPlaying] = useState(false);
  const nowRef = useRef(0);
  /** Target of a seek the element has not finished; its time stays on the playhead meanwhile. */
  const pending = useRef<number | null>(null);
  const ctx = useRef({ v, duration });
  ctx.current = { v, duration };

  const setNow = useCallback((t: number) => {
    nowRef.current = t;
    setNowState(t);
  }, []);

  useEffect(() => {
    setNow(0);
    pending.current = null;
  }, [info?.path, setNow]);

  useEffect(() => {
    setPlaying(false);
  }, [src]);

  const seek = useCallback(
    (t: number) => {
      const { v, duration } = ctx.current;
      // Land on the start of a frame, so the playhead (and IN / OUT taken from it) names the frame shown.
      const target = frameStart(Math.max(0, Math.min(duration > 0 ? duration : Number.POSITIVE_INFINITY, t)), v);
      setNow(target);
      pending.current = target;
      const el = ref.current;
      if (!el) return;
      const max = Number.isFinite(el.duration) ? el.duration : Number.POSITIVE_INFINITY;
      el.currentTime = Math.min(target + SEEK_EPS, max);
    },
    [ref, setNow],
  );

  const stepFrames = useCallback(
    (n: number) => {
      const { v } = ctx.current;
      ref.current?.pause();
      seek(stepFrame(nowRef.current, n, v));
    },
    [ref, seek],
  );

  const jump = useCallback((sec: number) => seek(nowRef.current + sec), [seek]);

  const pause = useCallback(() => ref.current?.pause(), [ref]);

  const toggle = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    if (!el.paused) {
      el.pause();
      return;
    }
    const { v, duration } = ctx.current;
    if (duration > 0 && nowRef.current >= duration - frameDuration(v)) seek(0);
    void el.play().catch(() => undefined);
  }, [ref, seek]);

  const restore = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    // Seeks asked before the source loaded (e.g. while the proxy was being made) never finished.
    if (nowRef.current > 0) {
      pending.current = nowRef.current;
      el.currentTime = nowRef.current + SEEK_EPS;
    } else {
      pending.current = null;
    }
  }, [ref]);

  // timeupdate fires only ~4 times a second; follow the frames while playing.
  useEffect(() => {
    const el = ref.current;
    if (!playing || !el) return;
    let handle = 0;
    const tick = () => {
      if (!el.seeking && pending.current === null) setNow(el.currentTime);
      handle = requestAnimationFrame(tick);
    };
    handle = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(handle);
  }, [playing, ref, setNow, src]);

  const events = {
    onPlay: () => setPlaying(true),
    onPause: () => {
      setPlaying(false);
      // Paused mid-frame: show the start of the frame on screen (exact IN / OUT from the playhead).
      const el = ref.current;
      if (!el || pending.current !== null) return;
      const t = frameStart(el.currentTime, ctx.current.v);
      setNow(t);
      // Paused within a frame of starting, WebKit can keep showing the frame before `currentTime`.
      // Seek to the frame the playhead names so the picture, the clock and I / O agree.
      if (t !== el.currentTime && Number.isFinite(el.duration) && t < el.duration) {
        pending.current = t;
        el.currentTime = t + SEEK_EPS;
      }
    },
    onEnded: () => setPlaying(false),
    onTimeUpdate: () => {
      const el = ref.current;
      if (el && !el.paused && !el.seeking && pending.current === null) setNow(el.currentTime);
    },
    onSeeked: () => {
      pending.current = null;
    },
  };

  return { now, playing, seek, stepFrames, jump, toggle, pause, restore, events };
}
