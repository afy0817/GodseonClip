// App shell: the editor first — or the FFmpeg settings when no usable FFmpeg was found at start —,
// 일괄 변환 behind the header's third work kind, the 작업 큐 behind the header's queue button, the
// FFmpeg settings behind the settings button, and from there the diagnostics (Phase 0 verification).
// All stay mounted so switching keeps what each had open; the queue state is shared.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { DiagnosticsScreen } from "./components/DiagnosticsScreen";
import { ConvertScreen } from "./features/convert/ConvertScreen";
import { EditorScreen } from "./features/editor/EditorScreen";
import type { EditorMode } from "./features/editor/exportOptions";
import type { WorkKind } from "./features/editor/TopBar";
import { activeCount } from "./features/queue/queueView";
import { QueueScreen } from "./features/queue/QueueScreen";
import { NotifyProvider } from "./features/notify/Notify";
import { SetupScreen } from "./features/setup/SetupScreen";
import type { FfmpegActions } from "./features/setup/setupView";
import { api, toMsg, type LocateResult, type PlatformInfo } from "./lib/ipc";
import { useQueue } from "./lib/useQueue";

type View = "editor" | "convert" | "queue" | "setup" | "diagnostics";

export default function App() {
  const [view, setView] = useState<View>("editor");
  const [platform, setPlatform] = useState<PlatformInfo | null>(null);
  const [locate, setLocate] = useState<LocateResult | null>(null);
  const [locating, setLocating] = useState(true);
  // 자르기 / 구간 제거: the other screens' header switches it as well.
  const [mode, setMode] = useState<EditorMode>("trim");
  const onKind = useCallback((k: WorkKind) => {
    if (k === "convert") {
      setView("convert");
    } else {
      setMode(k);
      setView("editor");
    }
  }, []);
  const queue = useQueue();
  const openQueue = useCallback(() => setView("queue"), []);
  const diagnosticsRef = useRef<HTMLDivElement>(null);
  // The first search (at start) decides the first screen.
  const started = useRef(false);

  const run = useCallback(async (request: () => Promise<LocateResult>) => {
    setLocating(true);
    let usable = false;
    try {
      const r = await request();
      usable = !!r.info?.meetsMinimum;
      setLocate(r);
    } catch (e) {
      setLocate((prev) => ({
        found: false,
        info: null,
        searched: [],
        problem: toMsg(e),
        minVersion: "6.0",
        savedPath: prev?.savedPath ?? null,
        notices: [],
      }));
    } finally {
      setLocating(false);
      // No usable FFmpeg at start: the app opens on the settings screen (install guide, own path).
      if (!started.current) {
        started.current = true;
        if (!usable) setView((v) => (v === "editor" ? "setup" : v));
      }
    }
  }, []);

  const ffmpegActions = useMemo<FfmpegActions>(
    () => ({
      locate: () => void run(api.locateFfmpeg),
      choose: (path) => void run(() => api.chooseFfmpegPath(path)),
      forget: () => void run(api.forgetFfmpegPath),
    }),
    [run],
  );

  // At start Rust tries the saved path first (settings.json), then the automatic search.
  useEffect(() => {
    api.platformInfo().then(setPlatform).catch(() => setPlatform(null));
    ffmpegActions.locate();
  }, [ffmpegActions]);

  // A hidden screen keeps playing sound otherwise.
  useEffect(() => {
    if (view !== "diagnostics") diagnosticsRef.current?.querySelectorAll("video").forEach((v) => v.pause());
    window.scrollTo(0, 0);
  }, [view]);

  // `problem` also explains an FFmpeg older than the minimum; `notices` a saved path that was skipped.
  const ffmpeg = {
    ready: !!locate?.info?.meetsMinimum,
    locating,
    problem: locate?.problem ?? null,
    notices: locate?.notices ?? [],
    path: locate?.info?.ffmpegPath ?? null,
  };

  return (
    <NotifyProvider queue={queue} openQueue={openQueue}>
      <div hidden={view !== "editor"}>
        <EditorScreen
          active={view === "editor"}
          ffmpeg={ffmpeg}
          mode={mode}
          onKind={onKind}
          queue={queue}
          queueCount={activeCount(queue.items)}
          onOpenQueue={() => setView("queue")}
          onOpenSettings={() => setView("setup")}
        />
      </div>
      <div hidden={view !== "convert"}>
        <ConvertScreen
          active={view === "convert"}
          ffmpeg={{ ...ffmpeg, info: ffmpeg.ready ? (locate?.info ?? null) : null }}
          queue={queue}
          onKind={onKind}
          onOpenQueue={() => setView("queue")}
          onOpenSettings={() => setView("setup")}
        />
      </div>
      <div hidden={view !== "queue"}>
        <QueueScreen active={view === "queue"} queue={queue} onKind={onKind} onOpenSettings={() => setView("setup")} />
      </div>
      <div hidden={view !== "setup"}>
        <SetupScreen
          active={view === "setup"}
          platform={platform}
          locate={locate}
          locating={locating}
          ffmpegActions={ffmpegActions}
          queue={queue}
          onKind={onKind}
          onOpenQueue={() => setView("queue")}
          onOpenEditor={() => setView("editor")}
          onOpenDiagnostics={() => setView("diagnostics")}
        />
      </div>
      <div hidden={view !== "diagnostics"} ref={diagnosticsRef}>
        <DiagnosticsScreen platform={platform} locate={locate} locating={locating} onOpenSetup={() => setView("setup")} />
      </div>
    </NotifyProvider>
  );
}
