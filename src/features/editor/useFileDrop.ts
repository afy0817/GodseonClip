// Files dragged onto the window from Finder / Explorer. Tauri hands over their paths through its own
// drag-drop events (webview option `dragDropEnabled`, on by default and set explicitly in
// tauri.conf.json); with it on, the webview's HTML5 drop never sees files and does not navigate to them.

import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useEffect, useRef, useState } from "react";

/**
 * While `enabled`: the paths being dragged over the window (null when none), and `onDrop` with the
 * dropped paths. Listens for the whole time the component is mounted.
 */
export function useFileDrop(enabled: boolean, onDrop: (paths: string[]) => void): string[] | null {
  const [dragged, setDragged] = useState<string[] | null>(null);
  const latest = useRef({ enabled, onDrop });
  latest.current = { enabled, onDrop };

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let gone = false;
    Promise.resolve()
      .then(() =>
        getCurrentWebview().onDragDropEvent(({ payload }) => {
          if (payload.type === "enter") {
            // Dragging text or a link from another app carries no paths.
            setDragged(payload.paths.length > 0 ? payload.paths : null);
          } else if (payload.type === "leave") {
            setDragged(null);
          } else if (payload.type === "drop") {
            setDragged(null);
            if (latest.current.enabled && payload.paths.length > 0) latest.current.onDrop(payload.paths);
          }
        }),
      )
      .then((stop) => {
        if (gone) stop();
        else unlisten = stop;
      })
      .catch((e) => console.error("drag-drop events unavailable", e));
    return () => {
      gone = true;
      unlisten?.();
    };
  }, []);

  return enabled ? dragged : null;
}
