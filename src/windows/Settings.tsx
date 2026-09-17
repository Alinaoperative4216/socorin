import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ipc, type PlatformInfo, type Settings as SettingsModel, type UpdateStatus } from "../lib/ipc";
import { isMac, prettyShortcut, shortcutFromEvent } from "../lib/hotkey";

type Status = { kind: "ok" | "error"; text: string } | null;

/** "today 09:15" / "17 Sep 2026" for the last update check. */
export function formatChecked(unixSeconds: number, now = new Date()): string {
  const d = new Date(unixSeconds * 1000);
  const sameDay = d.toDateString() === now.toDateString();
  const time = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  return sameDay ? `today ${time}` : d.toLocaleDateString([], { day: "numeric", month: "short", year: "numeric" });
}

function HotkeyField({
  value,
  onChange,
  placeholder,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder: string;
}) {
  const [recording, setRecording] = useState(false);
  const [hint, setHint] = useState<string | null>(null);
  const hintTimer = useRef<number | undefined>(undefined);

  const showHint = (text: string) => {
    setHint(text);
    window.clearTimeout(hintTimer.current);
    hintTimer.current = window.setTimeout(() => setHint(null), 2500);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    e.preventDefault();
    if (e.key === "Escape") {
      e.currentTarget.blur();
      return;
    }
    if (e.key === "Backspace" || e.key === "Delete") {
      onChange("");
      return;
    }
    const shortcut = shortcutFromEvent(e.nativeEvent);
    if (shortcut) {
      // Show the new combination right away and stop recording.
      onChange(shortcut);
      setHint(null);
      e.currentTarget.blur();
    } else if (!/^(Shift|Control|Alt|Meta|CapsLock)$/.test(e.key)) {
      showHint(isMac ? "Add a modifier: ⌘, ⌃, ⌥ or ⇧" : "Add a modifier: Ctrl, Alt, Shift or Win");
    }
  };

  return (
    <div className="hotkey-field">
      <input
        readOnly
        value={recording ? "" : prettyShortcut(value)}
        placeholder={recording ? (hint ?? "Press keys…") : placeholder}
        className={`${recording ? "recording" : ""} ${hint ? "invalid" : ""}`}
        onFocus={() => setRecording(true)}
        onBlur={() => {
          setRecording(false);
          setHint(null);
        }}
        onKeyDown={onKeyDown}
      />
      {value && !recording && (
        <button type="button" className="ghost" title="Clear" onClick={() => onChange("")}>
          ✕
        </button>
      )}
    </div>
  );
}

export function Settings() {
  const [settings, setSettings] = useState<SettingsModel | null>(null);
  const [platform, setPlatform] = useState<PlatformInfo | null>(null);
  const [status, setStatus] = useState<Status>(null);
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [updateInfo, setUpdateInfo] = useState<UpdateStatus | null>(null);
  const [checking, setChecking] = useState(false);
  const statusTimer = useRef<number | undefined>(undefined);
  const dirtyRef = useRef(false);
  dirtyRef.current = dirty;

  const flash = useCallback((s: Status, ms = 4000) => {
    setStatus(s);
    window.clearTimeout(statusTimer.current);
    if (s) statusTimer.current = window.setTimeout(() => setStatus(null), ms);
  }, []);

  const refreshPlatform = useCallback(() => {
    ipc.platformInfo().then(setPlatform).catch(console.error);
  }, []);

  useEffect(() => {
    ipc.getSettings().then(setSettings).catch((e) => flash({ kind: "error", text: String(e) }));
    refreshPlatform();
    ipc.updateStatus().then(setUpdateInfo).catch(() => {});
    const unlisten = [
      listen("screen-permission-missing", () => {
        refreshPlatform();
        flash({ kind: "error", text: "Screen Recording permission is required to take screenshots." }, 8000);
      }),
      listen<string>("capture-error", (e) => flash({ kind: "error", text: e.payload }, 8000)),
      listen<string>("capture-done", (e) =>
        flash({ kind: "ok", text: e.payload === "clipboard" ? "Copied to clipboard" : `Saved to ${e.payload}` }),
      ),
      listen<UpdateStatus>("update:status", (e) => setUpdateInfo(e.payload)),
    ];
    const onFocus = () => {
      refreshPlatform();
      // Other windows (overlay, welcome) save settings too; this window is
      // created hidden at startup, so its copy may be stale by the time it
      // is shown. Only reload while nothing is being edited here.
      if (!dirtyRef.current) ipc.getSettings().then(setSettings).catch(() => {});
    };
    window.addEventListener("focus", onFocus);
    return () => {
      unlisten.forEach((p) => p.then((f) => f()));
      window.removeEventListener("focus", onFocus);
    };
  }, [flash, refreshPlatform]);

  const update = (patch: Partial<SettingsModel>) => {
    setSettings((s) => (s ? { ...s, ...patch } : s));
    setDirty(true);
  };

  const save = async () => {
    if (!settings) return;
    setSaving(true);
    try {
      // Only the fields this window edits: the rest belong to other windows.
      const {
        hotkey,
        fullscreenHotkey,
        recordHotkey,
        saveDir,
        afterCapture,
        copyOnSave,
        autostart,
        cliTriggers,
        filePrefix,
        ffmpegPath,
        checkUpdates,
        autoUpdate,
      } = settings;
      const saved = await ipc.updateSettings({
        hotkey,
        fullscreenHotkey,
        recordHotkey,
        saveDir,
        afterCapture,
        copyOnSave,
        autostart,
        cliTriggers,
        filePrefix,
        ffmpegPath,
        checkUpdates,
        autoUpdate,
      });
      setSettings(saved);
      setDirty(false);
      flash({ kind: "ok", text: "Settings saved" });
    } catch (e) {
      flash({ kind: "error", text: String(e) }, 8000);
    } finally {
      setSaving(false);
    }
  };

  const chooseDir = async () => {
    const dir = await open({ directory: true, multiple: false, defaultPath: settings?.saveDir || undefined });
    if (typeof dir === "string") update({ saveDir: dir });
  };

  const resetDir = async () => update({ saveDir: await ipc.defaultSaveDir() });

  const checkNow = async () => {
    setChecking(true);
    try {
      const result = await ipc.checkForUpdates();
      setUpdateInfo(result);
      flash(
        result.available
          ? { kind: "ok", text: `Socorin ${result.available} is available` }
          : { kind: "ok", text: "Socorin is up to date" },
      );
    } catch (e) {
      flash({ kind: "error", text: String(e) }, 8000);
    } finally {
      setChecking(false);
    }
  };

  if (!settings) {
    return <div className="settings loading">Loading…</div>;
  }

  const needsPermission = platform?.os === "macos" && !platform.screenPermission;
  const installing = updateInfo?.phase.phase === "installing";
  const updateLine = !updateInfo
    ? ""
    : updateInfo.available
      ? `Version ${updateInfo.available} is available.`
      : updateInfo.checkedAt
        ? `Up to date, last checked ${formatChecked(updateInfo.checkedAt)}.`
        : "Not checked yet.";

  return (
    <div className="settings">
      <header className="settings-header">
        <h1>Socorin</h1>
        <p>Runs in the {isMac ? "menu bar" : "system tray"}. Press the shortcut anywhere to capture.</p>
      </header>

      {platform?.installIssue && (
        <section className="card warning">
          <h2>Move Socorin to Applications</h2>
          <p>
            {platform.installIssue === "translocated"
              ? "macOS is running a temporary, relocated copy of Socorin (Gatekeeper's app translocation). "
              : "Socorin is running from the disk image or an external volume. "}
            macOS will not grant the Screen Recording permission to an app there and usually does not even
            ask for it. Quit, drag <code>Socorin.app</code> into the Applications folder, eject the disk
            image and start it from there.
          </p>
        </section>
      )}

      {needsPermission && !platform?.installIssue && (
        <section className="card warning">
          <h2>Screen Recording permission</h2>
          <p>
            macOS requires permission before an app can capture the screen. Grant it under
            System Settings → Privacy &amp; Security → Screen Recording, then relaunch if needed.
          </p>
          <p className="hint">
            Socorin missing from that list, or the switch is on but capturing still fails (typical after an
            update)? Reset the stale entry in Terminal, then relaunch and press Request access again:
            <br />
            <code>tccutil reset ScreenCapture com.socorin</code>
          </p>
          <div className="row">
            <button type="button" onClick={() => ipc.requestScreenPermission().then(refreshPlatform)}>
              Request access
            </button>
            <button type="button" className="secondary" onClick={() => ipc.openScreenPermissionSettings()}>
              Open System Settings
            </button>
          </div>
        </section>
      )}

      {platform?.wayland && (
        <section className="card warning">
          <h2>Wayland session detected</h2>
          <p>
            Wayland does not allow apps to register global shortcuts. Bind a custom shortcut in your
            desktop's keyboard settings to the command <code>socorin --capture</code> instead, and turn on
            "Allow command-line triggers" under System.
          </p>
        </section>
      )}

      <section className="card">
        <h2>Shortcuts</h2>
        <label className="field">
          <span>Capture region</span>
          <HotkeyField value={settings.hotkey} onChange={(v) => update({ hotkey: v })} placeholder="Click and press keys" />
        </label>
        <label className="field">
          <span>Capture full screen</span>
          <HotkeyField
            value={settings.fullscreenHotkey}
            onChange={(v) => update({ fullscreenHotkey: v })}
            placeholder="Disabled"
          />
        </label>
        <label className="field">
          <span>Record region</span>
          <HotkeyField value={settings.recordHotkey} onChange={(v) => update({ recordHotkey: v })} placeholder="Disabled" />
        </label>
        <p className="hint">
          Click a field, then press the key combination. Backspace clears it. The record shortcut also stops a running
          recording.
        </p>
      </section>

      <section className="card">
        <h2>After capturing</h2>
        <div className="radio-group">
          {(
            [
              ["editor", "Annotate on screen (toolbar next to the selection)"],
              ["clipboard", "Copy to clipboard immediately"],
              ["save", "Save to folder immediately"],
            ] as const
          ).map(([value, text]) => (
            <label key={value} className="radio">
              <input
                type="radio"
                name="afterCapture"
                checked={settings.afterCapture === value}
                onChange={() => update({ afterCapture: value })}
              />
              <span>{text}</span>
            </label>
          ))}
        </div>
      </section>

      <section className="card">
        <h2>Saving</h2>
        <label className="field">
          <span>Folder</span>
          <div className="row grow">
            <input value={settings.saveDir} onChange={(e) => update({ saveDir: e.target.value })} />
            <button type="button" className="secondary" onClick={chooseDir}>
              Choose…
            </button>
            <button type="button" className="ghost" onClick={resetDir} title="Reset to default">
              Reset
            </button>
          </div>
        </label>
        <label className="field">
          <span>File name prefix</span>
          <input
            value={settings.filePrefix}
            onChange={(e) => update({ filePrefix: e.target.value })}
            placeholder="Socorin"
          />
        </label>
        <label className="checkbox">
          <input
            type="checkbox"
            checked={settings.copyOnSave}
            onChange={(e) => update({ copyOnSave: e.target.checked })}
          />
          <span>Also copy to clipboard when saving</span>
        </label>
        <p className="hint">
          Files are named <code>{settings.filePrefix || "Socorin"}_YYYY-MM-DD_HH-mm-ss.png</code>; screen recordings go to
          the same folder as <code>.{isMac ? "mov" : "mp4"}</code>.
        </p>
      </section>

      {!isMac && (
        <section className="card">
          <h2>Recording</h2>
          <label className="field">
            <span>ffmpeg</span>
            <input
              value={settings.ffmpegPath}
              onChange={(e) => update({ ffmpegPath: e.target.value })}
              placeholder="Found automatically"
            />
          </label>
          <p className="hint">
            Screen recording uses ffmpeg. Leave this empty to look in the usual install locations and on PATH, or
            enter the full path of <code>ffmpeg{platform?.os === "windows" ? ".exe" : ""}</code>.
          </p>
        </section>
      )}

      <section className="card">
        <h2>System</h2>
        <label className="checkbox">
          <input
            type="checkbox"
            checked={settings.autostart}
            onChange={(e) => update({ autostart: e.target.checked })}
          />
          <span>Launch at login</span>
        </label>
        <label className="checkbox">
          <input
            type="checkbox"
            checked={settings.cliTriggers}
            onChange={(e) => update({ cliTriggers: e.target.checked })}
          />
          <span>Allow command-line triggers</span>
        </label>
        <p className="hint">
          Lets <code>socorin --capture</code>, <code>--capture-full</code>, <code>--record</code> and{" "}
          <code>--record-full</code> start a capture (for shortcuts bound in your desktop environment). Off by
          default: with it on, any program on this computer can make Socorin take a screenshot for it.
        </p>
      </section>

      <section className="card">
        <h2>Updates</h2>
        <label className="checkbox">
          <input
            type="checkbox"
            checked={settings.checkUpdates}
            onChange={(e) => update({ checkUpdates: e.target.checked })}
          />
          <span>Check for updates automatically (once a day)</span>
        </label>
        <label className="checkbox">
          <input
            type="checkbox"
            checked={settings.autoUpdate}
            disabled={!settings.checkUpdates}
            onChange={(e) => update({ autoUpdate: e.target.checked })}
          />
          <span>Install updates automatically</span>
        </label>
        <p className="hint">
          A new version is announced next to the {isMac ? "menu bar" : "tray"} icon and in its menu. With automatic
          installs on, it is downloaded and installed right away and Socorin restarts by itself.
        </p>
        <div className="row update-row">
          <span className="update-line">
            Socorin {updateInfo?.currentVersion ?? ""}. {updateLine}
          </span>
          {updateInfo?.available && (
            <button type="button" disabled={installing} onClick={() => ipc.installUpdate()}>
              {installing ? "Updating…" : `Update to ${updateInfo.available}`}
            </button>
          )}
          <button type="button" className="secondary" disabled={checking || installing} onClick={checkNow}>
            {checking ? "Checking…" : "Check now"}
          </button>
        </div>
      </section>

      <footer className="settings-footer">
        <div className={`status ${status?.kind ?? ""}`}>{status?.text}</div>
        <div className="row">
          <button type="button" className="secondary" onClick={() => ipc.startCapture()}>
            Capture now
          </button>
          <button type="button" className="secondary" onClick={() => ipc.startRecordRegion()}>
            Record region
          </button>
          <button type="button" disabled={!dirty || saving} onClick={save}>
            {saving ? "Saving…" : "Save settings"}
          </button>
        </div>
        <div className="row footer-links">
          <button type="button" className="ghost" onClick={() => ipc.openSaveDir()}>
            Open screenshots folder
          </button>
          <button type="button" className="ghost danger" onClick={() => ipc.quit()}>
            Quit Socorin
          </button>
        </div>
        <p className="copyright">
          © {new Date().getFullYear()} Socorin ·{" "}
          <a
            href="https://socorin.com"
            onClick={(e) => {
              e.preventDefault();
              void openUrl("https://socorin.com");
            }}
          >
            socorin.com
          </a>
        </p>
      </footer>
    </div>
  );
}
