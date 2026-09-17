import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import logo from "../assets/logo.svg";
import { ipc, shareErrorMessage, type ShareNotice as Notice } from "../lib/ipc";

/** How long the popover stays without anyone touching it. */
export const LIFETIME_MS = 8000;

/** "expires in 60 days" / "expires in 1 day" / "expires today" from an ISO date. */
export function expiresIn(expiresAt: string, retentionDays: number, now = Date.now()): string {
  const at = Date.parse(expiresAt);
  const days = Number.isNaN(at) ? retentionDays : Math.ceil((at - now) / 86_400_000);
  if (days <= 0) return "expires today";
  return `expires in ${days} day${days === 1 ? "" : "s"}`;
}

/**
 * The small popover under the menu bar / tray icon after "Upload & copy
 * link": the link that is now on the clipboard (Copy again / Delete from
 * server), or why there is none. Rust shows the window once the first
 * notice is on screen (`shareReady`); it goes away on Close, Escape, a
 * click elsewhere, or by itself after `LIFETIME_MS` (a hover holds it).
 */
export function ShareNotice() {
  const [notice, setNotice] = useState<Notice | null>(null);
  const [status, setStatus] = useState<{ text: string; error?: boolean } | null>(null);
  const [busy, setBusy] = useState(false);
  const [deleted, setDeleted] = useState(false);
  const shown = useRef(false);
  const timer = useRef<number | undefined>(undefined);
  const hovering = useRef(false);

  const dismiss = useCallback(() => void ipc.dismissShare(), []);

  const arm = useCallback(() => {
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => {
      if (hovering.current) arm();
      else dismiss();
    }, LIFETIME_MS);
  }, [dismiss]);

  useEffect(() => {
    const fresh = (n: Notice | null | undefined) => {
      setNotice(n ?? null);
      setStatus(null);
      setDeleted(false);
      if (n) arm();
    };
    ipc.shareNotice().then(fresh).catch(() => {});
    const unlisten = listen<Notice>("share:notice", (e) => fresh(e.payload));
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        dismiss();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      unlisten.then((f) => f());
      window.removeEventListener("keydown", onKey);
      window.clearTimeout(timer.current);
    };
  }, [arm, dismiss]);

  useEffect(() => {
    if (notice && !shown.current) {
      shown.current = true;
      void ipc.shareReady();
    }
  }, [notice]);

  if (!notice) return null;

  const run = async (fn: () => Promise<void>) => {
    if (busy) return;
    setBusy(true);
    arm();
    try {
      await fn();
    } catch (e) {
      setStatus({ text: shareErrorMessage(e), error: true });
    } finally {
      setBusy(false);
    }
  };

  let body;
  if (notice.kind === "failed") {
    body = (
      <>
        <h1>Could not upload</h1>
        <p>{notice.error.message}</p>
        <div className="row">
          <button type="button" onClick={dismiss}>
            Close
          </button>
        </div>
      </>
    );
  } else if (deleted) {
    body = (
      <>
        <h1>Deleted from server</h1>
        <p>The link no longer works.</p>
        <div className="row">
          <button type="button" onClick={dismiss}>
            Close
          </button>
        </div>
      </>
    );
  } else {
    const { link, retentionDays } = notice;
    body = (
      <>
        <h1>Link copied · {expiresIn(link.expiresAt, retentionDays)}</h1>
        <code className="share-link" title={link.shareUrl}>
          {link.shareUrl}
        </code>
        <p>{status ? status.text : "Paste it anywhere. Anyone with the link can open the file until it expires."}</p>
        <div className="row">
          <button
            type="button"
            title="Copy the link again"
            disabled={busy}
            onClick={() =>
              void run(async () => {
                await ipc.copyShareLink(link.id);
                setStatus({ text: "Copied again." });
              })
            }
          >
            Copy
          </button>
          <button
            type="button"
            className="secondary danger"
            disabled={busy}
            onClick={() =>
              void run(async () => {
                await ipc.deleteShare(link.id);
                setDeleted(true);
              })
            }
          >
            {busy ? "Working…" : "Delete from server"}
          </button>
          <button type="button" className="secondary" onClick={dismiss}>
            Close
          </button>
        </div>
      </>
    );
  }

  return (
    <div
      className="update-notice share-notice"
      onMouseEnter={() => {
        hovering.current = true;
      }}
      onMouseLeave={() => {
        hovering.current = false;
        arm();
      }}
    >
      <img className="update-logo" src={logo} alt="" width={40} height={40} draggable={false} />
      <div className="update-body">{body}</div>
    </div>
  );
}
