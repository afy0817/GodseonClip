import { useEffect, useState } from "react";
import { api, toMsg, type ExportEncoders, type Msg } from "../../lib/ipc";

export interface ExportEncodersState {
  encoders: ExportEncoders | null;
  loading: boolean;
  error: Msg | null;
}

/**
 * The encoder each precise export would use. Asked when the precise mode is chosen, again for
 * another FFmpeg and on coming back from the settings screen (where the encoders may have been
 * re-tested). Rust caches the three-frame test, so only the first ask takes a few seconds.
 */
export function useExportEncoders(wanted: boolean, ffmpegPath: string | null, active: boolean): ExportEncodersState {
  const [state, setState] = useState<ExportEncodersState>({ encoders: null, loading: false, error: null });

  useEffect(() => {
    if (!wanted || !ffmpegPath || !active) return;
    let alive = true;
    setState((s) => ({ ...s, loading: true, error: null }));
    api
      .exportEncoders()
      .then((encoders) => alive && setState({ encoders, loading: false, error: null }))
      .catch((e) => alive && setState({ encoders: null, loading: false, error: toMsg(e) }));
    return () => {
      alive = false;
    };
  }, [wanted, ffmpegPath, active]);

  return state;
}
