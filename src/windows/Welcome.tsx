import { useEffect, useState } from "react";
import logo from "../assets/logo.svg";
import { ipc } from "../lib/ipc";
import { isMac, prettyShortcut } from "../lib/hotkey";

/**
 * First-launch dialog: the app has no Dock icon and no main window, so tell
 * the user where it lives. OK records `welcomeShown`; the dialog never comes
 * back after that.
 */
export function Welcome() {
  const [hotkey, setHotkey] = useState("");

  useEffect(() => {
    // Show the window only once the hotkey is known, so nothing re-flows.
    ipc.getSettings()
      .then((s) => setHotkey(s.hotkey))
      .catch(() => {})
      .finally(() => void ipc.welcomeReady());
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Enter" || e.key === "Escape") {
        e.preventDefault();
        void ipc.dismissWelcome();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const where = isMac ? "menu bar" : "system tray";
  const spot = isMac ? "at the top right of your screen" : "next to the clock";

  return (
    <div className="welcome">
      <img className="welcome-logo" src={logo} alt="" width={96} height={96} draggable={false} />
      <h1>Welcome to Socorin</h1>
      <p>
        Socorin runs in the {where}. Look for the S icon {spot} — there is no Dock icon and no main window.
      </p>
      <p>
        Press <kbd>{prettyShortcut(hotkey) || "the shortcut"}</kbd> anywhere to capture a region, or click the S
        icon for full-screen capture and Settings.
      </p>
      <p className="welcome-note">This message is shown only once.</p>
      <button type="button" className="welcome-ok" autoFocus onClick={() => void ipc.dismissWelcome()}>
        OK
      </button>
    </div>
  );
}
