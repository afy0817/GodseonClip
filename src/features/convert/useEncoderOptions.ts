import { useCallback, useEffect, useState } from "react";
import { api, toMsg, type EncoderOptions, type Msg } from "../../lib/ipc";

export interface EncoderOptionsState {
  options: EncoderOptions | null;
  loading: boolean;
  error: Msg | null;
  /** "다시 검사": runs the three-frame encoder test again, then lists the encoders anew. */
  retest: () => void;
}

/**
 * The 일괄 변환 인코더 list. Asked when the screen is shown, again for another FFmpeg and on coming
 * back from the settings screen (where the encoders may have been re-tested). Rust caches the test,
 * so only the first ask takes a few seconds.
 */
export function useEncoderOptions(active: boolean, ffmpegPath: string | null): EncoderOptionsState {
  const [options, setOptions] = useState<EncoderOptions | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<Msg | null>(null);

  const load = useCallback((ask: () => Promise<unknown>) => {
    let alive = true;
    setLoading(true);
    setError(null);
    ask()
      .then(() => api.encoderOptions())
      .then((o) => alive && setOptions(o))
      .catch((e) => alive && (setOptions(null), setError(toMsg(e))))
      .finally(() => alive && setLoading(false));
    return () => {
      alive = false;
    };
  }, []);

  useEffect(() => {
    if (!active || !ffmpegPath) return;
    return load(() => Promise.resolve());
  }, [active, ffmpegPath, load]);

  const retest = useCallback(() => void load(() => api.testEncoders()), [load]);
  return { options, loading, error, retest };
}
