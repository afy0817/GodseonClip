import { open } from "@tauri-apps/plugin-dialog";
import type { Messages } from "./i18n/ko";

export const VIDEO_EXTENSIONS = ["mp4", "mov", "m4v", "mkv", "webm", "avi", "mts", "m2ts", "ts", "wmv", "flv", "3gp"];

/** System file dialog for one video file. Resolves to its path, or null when dismissed. */
export async function pickVideoFile(t: Messages): Promise<string | null> {
  const picked = await open({
    multiple: false,
    directory: false,
    title: t.files.openVideo,
    filters: [{ name: t.files.videoFilter, extensions: VIDEO_EXTENSIONS }],
  });
  return typeof picked === "string" ? picked : null;
}

export const IMAGE_EXTENSIONS = ["png", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff"];

/** System file dialog for a video or image to put over the picture (PiP). */
export async function pickOverlayFile(t: Messages): Promise<string | null> {
  const picked = await open({
    multiple: false,
    directory: false,
    title: t.overlays.pick,
    filters: [{ name: t.overlays.filter, extensions: [...VIDEO_EXTENSIONS, ...IMAGE_EXTENSIONS] }],
  });
  return typeof picked === "string" ? picked : null;
}

/** Last path component, for macOS / Linux and Windows paths alike. */
export function fileNameOf(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}
