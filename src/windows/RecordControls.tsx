import type { MouseEvent as ReactMouseEvent } from "react";
import { Check, ClipboardCopy, Disc, Square, X } from "lucide-react";

/**
 * What the bar is doing. `ready`: the area is selected, Record / Cancel.
 * `recording` (and `starting`, before the encoder is up): the clock and
 * Stop & copy / Stop / Cancel. `stopping`: a stop is in flight. `copied`:
 * the file is on the clipboard, the bar is about to go.
 */
export type RecordPhase = "ready" | "starting" | "recording" | "stopping" | "copied";

interface Props {
  phase: RecordPhase;
  /** Milliseconds recorded so far. */
  elapsedMs: number;
  onRecord?: () => void;
  onStop?: () => void;
  onStopCopy?: () => void;
  onCancel?: () => void;
}

/** `mm:ss`, or `h:mm:ss` from the first hour on (same as the tray). */
export function formatElapsed(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  const p = (n: number) => String(n).padStart(2, "0");
  const h = Math.floor(s / 3600);
  return `${h ? `${h}:` : ""}${p(Math.floor((s % 3600) / 60))}:${p(s % 60)}`;
}

/**
 * The recording toolbar. One component draws it before the recording (on
 * the overlay) and during it (in its own window placed at the same spot),
 * with a fixed width and fixed button slots, so nothing moves between the
 * two: Record becomes Stop, Cancel stays Cancel.
 */
export function RecordControls({ phase, elapsedMs, onRecord, onStop, onStopCopy, onCancel }: Props) {
  // Buttons must not take keyboard focus (Enter / Esc are handled by the owner).
  const stopFocus = (e: ReactMouseEvent) => e.preventDefault();
  const busy = phase === "stopping" || phase === "copied";

  return (
    <div className={`toolbar floating record-bar phase-${phase}`} onMouseDown={stopFocus}>
      <div className="toolbar-row">
        {phase === "ready" ? (
          <span className="record-hint">Adjust the area, then</span>
        ) : phase === "copied" ? (
          <span className="record-status">
            <Check size={16} className="rec-ok" /> Copied to clipboard
          </span>
        ) : (
          <span className="record-status">
            <span className={`rec-dot ${phase === "recording" ? "live" : ""}`} />
            <span className="rec-time">{formatElapsed(elapsedMs)}</span>
          </span>
        )}
        <span className="spacer" />
        {phase === "ready" ? (
          <>
            <button type="button" className="tool-btn wide record" title="Start recording (Enter)" onClick={onRecord}>
              <Disc size={16} /> Record
            </button>
            <button type="button" className="tool-btn wide cancel" title="Cancel (Esc)" onClick={onCancel}>
              <X size={16} /> Cancel
            </button>
          </>
        ) : (
          <>
            <button
              type="button"
              className="tool-btn wide stop-copy"
              title="Stop and copy the video to the clipboard"
              disabled={busy}
              onClick={onStopCopy}
            >
              <ClipboardCopy size={16} /> Stop &amp; copy
            </button>
            <button type="button" className="tool-btn wide stop" title="Stop recording" disabled={busy} onClick={onStop}>
              <Square size={13} fill="currentColor" /> Stop
            </button>
            <button type="button" className="tool-btn wide cancel" title="Discard the recording" disabled={busy} onClick={onCancel}>
              <X size={16} /> Cancel
            </button>
          </>
        )}
      </div>
    </div>
  );
}
