import { useT } from "../../lib/i18n/I18n";
import { formatTime } from "../../lib/time";
import type { EditorMode } from "./exportOptions";
import { BackFrameIcon, BackSecondIcon, ForwardFrameIcon, ForwardSecondIcon, PauseIcon, PlayIcon, ScissorsIcon } from "./icons";
import type { Player } from "./usePlayer";

interface Props {
  player: Player;
  duration: number;
  enabled: boolean;
  mode: EditorMode;
  onMarkIn: () => void;
  onMarkOut: () => void;
  /** 구간 제거: add the marked range; null while there is nothing to add. */
  onAdd: (() => void) | null;
}

/**
 * Play / step buttons, the clock, and from the playhead: IN / OUT (자르기) or the start / end of
 * the next range to remove and 추가 (구간 제거). Shortcuts: EditorScreen.
 */
export function Transport({ player, duration, enabled, mode, onMarkIn, onMarkOut, onAdd }: Props) {
  const w = useT().transport;
  const cut = mode === "cut";
  return (
    <div className="ed-transport">
      <div className="ed-tbtns">
        <button
          type="button"
          className="ed-tbtn"
          aria-label={w.back1s}
          title={w.back1sTitle}
          disabled={!enabled}
          onClick={() => player.jump(-1)}
        >
          <BackSecondIcon />
        </button>
        <button
          type="button"
          className="ed-tbtn"
          aria-label={w.back1f}
          title={w.back1fTitle}
          disabled={!enabled}
          onClick={() => player.stepFrames(-1)}
        >
          <BackFrameIcon />
        </button>
        <button
          type="button"
          className="ed-play"
          aria-label={player.playing ? w.pause : w.play}
          title={player.playing ? w.pauseTitle : w.playTitle}
          disabled={!enabled}
          onClick={player.toggle}
        >
          {player.playing ? <PauseIcon /> : <PlayIcon />}
        </button>
        <button
          type="button"
          className="ed-tbtn"
          aria-label={w.fwd1f}
          title={w.fwd1fTitle}
          disabled={!enabled}
          onClick={() => player.stepFrames(1)}
        >
          <ForwardFrameIcon />
        </button>
        <button
          type="button"
          className="ed-tbtn"
          aria-label={w.fwd1s}
          title={w.fwd1sTitle}
          disabled={!enabled}
          onClick={() => player.jump(1)}
        >
          <ForwardSecondIcon />
        </button>
      </div>

      <div className="ed-clock mono" aria-label={w.playhead}>
        <span>{formatTime(player.now)}</span>
        <span className="ed-clock-total"> / {formatTime(duration)}</span>
      </div>

      <div className="ed-marks">
        <button
          type="button"
          className={cut ? "ed-btn" : "ed-btn mark"}
          disabled={!enabled}
          onClick={onMarkIn}
          aria-keyshortcuts="I"
          title={cut ? w.markStartTitle : w.markInTitle}
        >
          {cut ? w.markStart : w.markIn} <kbd className="ed-kbd">I</kbd>
        </button>
        <button
          type="button"
          className={cut ? "ed-btn" : "ed-btn mark"}
          disabled={!enabled}
          onClick={onMarkOut}
          aria-keyshortcuts="O"
          title={cut ? w.markEndTitle : w.markOutTitle}
        >
          {cut ? w.markEnd : w.markOut} <kbd className="ed-kbd">O</kbd>
        </button>
        {cut && (
          <button
            type="button"
            className="ed-btn cut"
            disabled={!enabled || !onAdd}
            onClick={onAdd ?? undefined}
            aria-keyshortcuts="A"
            title={w.addTitle}
          >
            <ScissorsIcon />
            {w.add} <kbd className="ed-kbd">A</kbd>
          </button>
        )}
      </div>
    </div>
  );
}
