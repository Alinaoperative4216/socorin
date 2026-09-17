import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { ipc, RECORDER_MARGIN, type RecordingStatus, type RecordingStopped } from "../lib/ipc";
import type { Anchor } from "../lib/ipc";
import { RecordControls, type RecordPhase } from "./RecordControls";

/**
 * The floating bar shown while a region records: its own small transparent
 * always-on-top window that Rust puts exactly where the overlay's Record
 * bar was, so the buttons stay put. Shows the elapsed time and
 * Stop & copy / Stop & upload / Stop / Cancel; says "Copied to clipboard"
 * or "Link copied" for a moment after Stop & copy / Stop & upload, and
 * "Uploading…" while the file is on its way.
 */
export function Recorder() {
  const [status, setStatus] = useState<RecordingStatus | null>(null);
  const [phase, setPhase] = useState<RecordPhase>("starting");
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    ipc.recordingStatus()
      .then((s) => {
        if (s.recording) {
          setStatus(s);
          setPhase("recording");
        }
      })
      .catch(() => {});
    const unlisten = [
      // A new recording claimed the bar (it is shown before the encoder runs).
      listen("recording:reset", () => {
        setStatus(null);
        setPhase("starting");
      }),
      listen<RecordingStatus>("recording:started", (e) => {
        setStatus(e.payload);
        setPhase("recording");
      }),
      listen("recording:uploading", () => {
        setStatus(null);
        setPhase("uploading");
      }),
      listen<RecordingStopped>("recording:stopped", (e) => {
        setStatus(null);
        setPhase(e.payload?.link ? "shared" : e.payload?.copied ? "copied" : "starting");
      }),
    ];
    const timer = window.setInterval(() => setNow(Date.now()), 250);
    return () => {
      unlisten.forEach((p) => p.then((f) => f()));
      window.clearInterval(timer);
    };
  }, []);

  const elapsed = status ? Math.max(0, now - status.startedMs) : 0;
  const stop = (action: (anchor?: Anchor) => Promise<void>) => (anchor?: Anchor) => {
    setPhase("stopping");
    void action(anchor).catch(() => setPhase("recording"));
  };

  return (
    <div className="recorder-window">
      <div className="floating-toolbar" style={{ left: RECORDER_MARGIN, top: RECORDER_MARGIN }}>
        <RecordControls
          phase={phase}
          elapsedMs={elapsed}
          onStop={stop(ipc.stopRecording)}
          onStopCopy={stop(ipc.stopRecordingCopy)}
          onStopUpload={stop(ipc.stopRecordingUpload)}
          onCancel={stop(ipc.cancelRecording)}
        />
      </div>
    </div>
  );
}
