import { memo, useEffect, useRef } from "react";
import type { Waveform } from "../../lib/ipc";

interface Props {
  wave: Waveform | null;
  /** Length of the whole timeline the canvas spans, in seconds. */
  duration: number;
  className?: string;
}

/** Widest canvas drawn at full resolution (Chromium's limit is 32767; wider ones are stretched). */
const MAX_CANVAS_PX = 16384;

/**
 * Mirrored peak bars, one column per device pixel (the max of the buckets it covers).
 * Colour: the `--wave-color` custom property, else `--blue`.
 */
export const WaveCanvas = memo(function WaveCanvas({ wave, duration, className = "wave" }: Props) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const canvas = ref.current;
    if (!canvas) return;
    const draw = () => {
      const dpr = window.devicePixelRatio || 1;
      const w = Math.max(1, Math.min(MAX_CANVAS_PX, Math.round(canvas.clientWidth * dpr)));
      const h = Math.max(1, Math.round(canvas.clientHeight * dpr));
      canvas.width = w;
      canvas.height = h;
      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      ctx.clearRect(0, 0, w, h);
      if (!wave || wave.peaks.length === 0 || duration <= 0) return;
      const style = getComputedStyle(canvas);
      ctx.fillStyle =
        style.getPropertyValue("--wave-color").trim() || style.getPropertyValue("--blue").trim() || "#6ea0ff";
      const n = wave.peaks.length;
      const x0 = (wave.start / duration) * w;
      const x1 = (wave.end / duration) * w;
      for (let x = Math.floor(x0); x < Math.ceil(x1); x++) {
        const a = Math.max(0, Math.floor(((x - x0) / (x1 - x0)) * n));
        const b = Math.min(n, Math.max(a + 1, Math.floor(((x + 1 - x0) / (x1 - x0)) * n)));
        let peak = 0;
        for (let i = a; i < b; i++) peak = Math.max(peak, wave.peaks[i]);
        const bar = Math.max(1, peak * h);
        ctx.fillRect(x, (h - bar) / 2, 1, bar);
      }
    };
    draw();
    const observer = new ResizeObserver(draw);
    observer.observe(canvas);
    return () => observer.disconnect();
  }, [wave, duration]);
  return <canvas ref={ref} className={className} aria-hidden="true" />;
});
