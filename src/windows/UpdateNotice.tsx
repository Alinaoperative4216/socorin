import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import logo from "../assets/logo.svg";
import { DOWNLOAD_URL, ipc, type UpdateStatus } from "../lib/ipc";

/** "3.2 MB" for the progress line. */
export function formatBytes(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)} MB`;
  if (n >= 1_000) return `${Math.round(n / 1_000)} KB`;
  return `${n} B`;
}

/**
 * The small popover under the menu bar / tray icon: a newer version with
 * Update / Later, the progress of installing it, a failure with the download
 * page, or "updated" once after an update. Rust positions and shows the
 * window once the first status is on screen (`updateReady`).
 */
export function UpdateNotice() {
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const shown = useRef(false);

  useEffect(() => {
    ipc.updateStatus().then(setStatus).catch(() => {});
    const unlisten = listen<UpdateStatus>("update:status", (e) => setStatus(e.payload));
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        void ipc.dismissUpdate();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      unlisten.then((f) => f());
      window.removeEventListener("keydown", onKey);
    };
  }, []);

  useEffect(() => {
    if (status && !shown.current) {
      shown.current = true;
      void ipc.updateReady();
    }
  }, [status]);

  if (!status) return null;

  const dismiss = () => void ipc.dismissUpdate();
  const { phase } = status;
  let body;
  if (phase.phase === "installing") {
    const percent = phase.total > 0 ? Math.min(100, Math.round((phase.received / phase.total) * 100)) : null;
    body = (
      <>
        <h1>Updating to Socorin {status.available}…</h1>
        <div className="progress" role="progressbar" aria-valuenow={percent ?? undefined}>
          <div className={percent === null ? "indeterminate" : ""} style={{ width: `${percent ?? 30}%` }} />
        </div>
        <p>
          {percent === null ? `${formatBytes(phase.received)} downloaded` : `${percent}% – ${formatBytes(phase.received)} of ${formatBytes(phase.total)}`}
          . Socorin restarts by itself when done.
        </p>
      </>
    );
  } else if (phase.phase === "failed") {
    body = (
      <>
        <h1>Could not update Socorin</h1>
        <p>{phase.error}</p>
        <div className="row">
          <button
            type="button"
            onClick={() => {
              void openUrl(DOWNLOAD_URL);
              dismiss();
            }}
          >
            Open download page
          </button>
          <button type="button" className="secondary" onClick={dismiss}>
            Close
          </button>
        </div>
      </>
    );
  } else if (status.justUpdated) {
    body = (
      <>
        <h1>Socorin was updated to {status.currentVersion}</h1>
        <p>You are now running the latest version.</p>
        <div className="row">
          <button type="button" autoFocus onClick={dismiss}>
            OK
          </button>
        </div>
      </>
    );
  } else if (status.available) {
    body = (
      <>
        <h1>Socorin {status.available} is available</h1>
        <p>You have {status.currentVersion}. Updating downloads the new version, installs it and restarts Socorin.</p>
        <div className="row">
          <button type="button" onClick={() => void ipc.installUpdate()}>
            Update now
          </button>
          <button type="button" className="secondary" onClick={dismiss}>
            Later
          </button>
        </div>
      </>
    );
  } else {
    body = (
      <>
        <h1>Socorin is up to date</h1>
        <p>Version {status.currentVersion} is the latest.</p>
        <div className="row">
          <button type="button" onClick={dismiss}>
            OK
          </button>
        </div>
      </>
    );
  }

  return (
    <div className="update-notice">
      <img className="update-logo" src={logo} alt="" width={40} height={40} draggable={false} />
      <div className="update-body">{body}</div>
    </div>
  );
}
