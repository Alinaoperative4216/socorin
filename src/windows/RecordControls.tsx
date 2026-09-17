import type { MouseEvent as ReactMouseEvent } from "react";
import { Check, ClipboardCopy, CloudUpload, Disc, Square, X } from "lucide-react";
import { anchorOf, type Anchor } from "../lib/ipc";
import { ToolbarLogo } from "../editor/Toolbar";

/**
 * What the bar is doing. `ready`: the area is selected, Record / Cancel.
 * `recording` (and `starting`, before the encoder is up): the clock and
 * Stop & copy / Stop & upload / Stop / Cancel. `stopping`: a stop is in
 * flight. `uploading`: the file is on its way to the share server.
 * `copied` / `shared`: the file or its link is on the clipboard, the bar
 * is about to go.
 */
export type RecordPhase = "ready" | "starting" | "recording" | "stopping" | "uploading" | "copied" | "shared";

interface Props {
  phase: RecordPhase;
  /** Milliseconds recorded so far. */
  elapsedMs: number;
  onRecord?: () => void;
  onStop?: () => void;
  onStopCopy?: () => void;
  /** `anchor`: the button, where the "Link copied" popover goes. */
  onStopUpload?: (anchor?: Anchor) => void;
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
export function RecordControls({ phase, elapsedMs, onRecord, onStop, onStopCopy, onStopUpload, onCancel }: Props) {
  // Buttons must not take keyboard focus (Enter / Esc are handled by the owner).
  const stopFocus = (e: ReactMouseEvent) => e.preventDefault();
  const busy = phase === "stopping" || phase === "uploading" || phase === "copied" || phase === "shared";

  return (
    <div className={`toolbar floating record-bar phase-${phase}`} onMouseDown={stopFocus}>
      <div className="toolbar-row">
        <ToolbarLogo />
        {phase === "ready" ? (
          <span className="record-hint">Adjust the area, then</span>
        ) : phase === "copied" ? (
          <span className="record-status">
            <Check size={16} className="rec-ok" /> Copied to clipboard
          </span>
        ) : phase === "shared" ? (
          <span className="record-status">
            <Check size={16} className="rec-ok" /> Link copied
          </span>
        ) : phase === "uploading" ? (
          <span className="record-status">
            <CloudUpload size={16} className="rec-busy" /> Uploading…
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
            <button
              type="button"
              className="tool-btn wide stop-upload"
              title="Stop and upload the video, copying its link"
              disabled={busy}
              onClick={(e) => onStopUpload?.(anchorOf(e.currentTarget))}
            >
              <CloudUpload size={16} /> Stop &amp; upload
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
