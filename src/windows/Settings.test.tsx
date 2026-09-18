import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { PlatformInfo } from "../lib/ipc";
import { isMac, prettyShortcut } from "../lib/hotkey";
import { flush, installTauri, SETTINGS, UPDATE_STATUS, type TauriMock } from "../test/tauri";
import { describeLink, formatChecked, Settings } from "./Settings";

let tauri: TauriMock;
const platform = (extra: Partial<PlatformInfo> = {}): PlatformInfo => ({
  os: "macos",
  screenPermission: true,
  micPermission: "granted",
  wayland: false,
  installIssue: null,
  ...extra,
});

beforeEach(() => {
  tauri = installTauri("main", {
    get_settings: () => ({ ...SETTINGS, fullscreenHotkey: "F5" }),
    platform_info: () => platform(),
    update_settings: (args) => ({ ...SETTINGS, ...(args as { patch: object }).patch }),
    default_save_dir: () => "/Users/me/Pictures/Screenshots",
    update_status: () => UPDATE_STATUS,
    share_history: () => [],
    audio_inputs: () => MICS,
  });
});

const MICS = [
  { id: "builtin", name: "MacBook Pro Microphone", default: true },
  { id: "usb:1", name: "Jabra Speak 710", default: false },
];

const LINK = {
  id: "med-0123456789abcdefgh",
  shareUrl: "https://socorin.com/s/med-0123456789abcdefgh",
  expiresAt: "2026-11-17T00:00:00Z",
  kind: "image" as const,
  mime: "image/png",
  size: 1_300_000,
  createdAt: 1_758_000_000,
};

async function mountLoaded() {
  const view = render(<Settings />);
  expect(screen.getByText("Loading…")).toBeTruthy();
  await screen.findByText("Shortcuts");
  return view;
}

const hotkeyInputs = (container: HTMLElement) => Array.from(container.querySelectorAll(".hotkey-field input")) as HTMLInputElement[];
const saveButton = () => screen.getByText(/Save settings|Saving…/) as HTMLButtonElement;

describe("Settings window", () => {
  it("shows the current settings and no warnings on a healthy setup", async () => {
    const { container } = await mountLoaded();
    const [region, full, record] = hotkeyInputs(container);
    expect(region.value).toBe(prettyShortcut("CmdOrCtrl+Shift+A"));
    expect(full.value).toBe("F5");
    expect(record.value).toBe("");
    expect(record.placeholder).toBe("Disabled");
    expect(screen.queryByText(/Screen Recording permission/)).toBeNull();
    expect(screen.queryByText(/Wayland/)).toBeNull();
    expect(saveButton().disabled).toBe(true);
    expect(screen.getByText(/Socorin_YYYY-MM-DD_HH-mm-ss\.png/)).toBeTruthy();
    // Launch at login is on until the user turns it off; the CLI hint names the binary.
    expect((screen.getByLabelText("Launch at login") as HTMLInputElement).checked).toBe(true);
    expect(screen.getByText("socorin --capture")).toBeTruthy();
  });

  it("records a shortcut, hints on modifier-less keys and clears with Backspace or ✕", async () => {
    const { container } = await mountLoaded();
    vi.useFakeTimers();
    const [region] = hotkeyInputs(container);
    act(() => region.focus());
    expect(region.placeholder).toBe("Press keys…");
    expect(region.value).toBe("");
    expect(region.className).toContain("recording");

    fireEvent.keyDown(region, { key: "a", code: "KeyA" });
    expect(region.placeholder).toMatch(/Add a modifier/);
    expect(region.className).toContain("invalid");
    fireEvent.keyDown(region, { key: "Shift", code: "ShiftLeft", shiftKey: true });
    await act(() => vi.advanceTimersByTimeAsync(2500));
    expect(region.placeholder).toBe("Press keys…");

    fireEvent.keyDown(region, { key: "k", code: "KeyK", shiftKey: true, altKey: true });
    // Committed immediately and recording stopped.
    expect(region.value).toBe(prettyShortcut("Alt+Shift+K"));
    expect(saveButton().disabled).toBe(false);

    act(() => region.focus());
    fireEvent.keyDown(region, { key: "Backspace" });
    act(() => region.blur());
    expect(region.value).toBe("");
    const [, full] = hotkeyInputs(container);
    act(() => full.focus());
    fireEvent.keyDown(full, { key: "Escape" });
    act(() => full.blur());
    expect(full.value).toBe("F5");
    fireEvent.click(screen.getAllByTitle("Clear")[0]);
    expect(hotkeyInputs(container)[1].value).toBe("");
  });

  it("saves only the fields it owns and reports the outcome", async () => {
    const { container } = await mountLoaded();
    vi.useFakeTimers();
    fireEvent.click(screen.getByLabelText("Copy to clipboard immediately"));
    fireEvent.change(container.querySelector("input[value='/tmp/shots']")!, { target: { value: "/new/dir" } });
    fireEvent.change(screen.getByPlaceholderText("Socorin"), { target: { value: "Shot" } });
    fireEvent.click(screen.getByLabelText("Also copy to clipboard when saving"));
    fireEvent.click(screen.getByLabelText("Launch at login"));
    fireEvent.click(screen.getByLabelText("Allow command-line triggers"));
    fireEvent.click(screen.getByLabelText("Install updates automatically"));
    expect(screen.getByText(/Shot_YYYY-MM-DD/)).toBeTruthy();

    fireEvent.click(saveButton());
    expect(saveButton().textContent).toBe("Saving…");
    await act(() => vi.advanceTimersByTimeAsync(1));
    const patch = (tauri.calls("update_settings")[0] as { patch: Record<string, unknown> }).patch;
    expect(patch).toMatchObject({
      hotkey: "CmdOrCtrl+Shift+A",
      fullscreenHotkey: "F5",
      recordHotkey: "",
      saveDir: "/new/dir",
      afterCapture: "clipboard",
      copyOnSave: false,
      autostart: false,
      cliTriggers: true,
      filePrefix: "Shot",
      checkUpdates: true,
      autoUpdate: false,
      uploadServer: "https://socorin.com",
    });
    // The style and welcome flags belong to other windows, the update
    // bookkeeping to Rust.
    expect(patch).not.toHaveProperty("annotationColor");
    expect(patch).not.toHaveProperty("welcomeShown");
    expect(patch).not.toHaveProperty("updateCheckedAt");
    expect(patch).not.toHaveProperty("updateAvailable");
    expect(screen.getByText("Settings saved").className).toContain("ok");
    expect(saveButton().disabled).toBe(true);
    await act(() => vi.advanceTimersByTimeAsync(4000));
    expect(screen.queryByText("Settings saved")).toBeNull();

    tauri.handlers.update_settings = () => Promise.reject("cannot register \"F5\"");
    fireEvent.click(screen.getByLabelText("Save to folder immediately"));
    fireEvent.click(saveButton());
    await act(() => vi.advanceTimersByTimeAsync(1));
    expect(screen.getByText('cannot register "F5"').className).toContain("error");
  });

  it("chooses and resets the save folder", async () => {
    tauri.handlers["plugin:dialog|open"] = () => "/picked/folder";
    const { container } = await mountLoaded();
    fireEvent.click(screen.getByText("Choose…"));
    await waitFor(() => expect(container.querySelector("input[value='/picked/folder']")).toBeTruthy());
    const dialogArgs = tauri.calls("plugin:dialog|open")[0] as { options: { directory: boolean; defaultPath: string } };
    expect(dialogArgs.options).toMatchObject({ directory: true, defaultPath: "/tmp/shots" });

    tauri.handlers["plugin:dialog|open"] = () => null;
    fireEvent.click(screen.getByText("Choose…"));
    await flush();
    expect(container.querySelector("input[value='/picked/folder']")).toBeTruthy();

    fireEvent.click(screen.getByTitle("Reset to default"));
    await waitFor(() => expect(container.querySelector("input[value='/Users/me/Pictures/Screenshots']")).toBeTruthy());
  });

  it("reacts to capture events and reloads on focus while clean", async () => {
    await mountLoaded();
    act(() => tauri.emit("capture-done", "clipboard"));
    expect(screen.getByText("Copied to clipboard")).toBeTruthy();
    act(() => tauri.emit("capture-done", "/tmp/a.png"));
    expect(screen.getByText("Saved to /tmp/a.png")).toBeTruthy();
    act(() => tauri.emit("capture-error", "something broke"));
    expect(screen.getByText("something broke").className).toContain("error");

    tauri.handlers.platform_info = () => platform({ screenPermission: false });
    act(() => tauri.emit("screen-permission-missing"));
    await screen.findByText("Screen Recording permission");
    expect(screen.getByText(/permission is required/)).toBeTruthy();

    tauri.handlers.get_settings = () => ({ ...SETTINGS, filePrefix: "Other" });
    fireEvent.focus(window);
    await screen.findByText(/Other_YYYY-MM-DD/);

    // Dirty: focus must not clobber the edits.
    fireEvent.change(screen.getByPlaceholderText("Socorin"), { target: { value: "Mine" } });
    tauri.handlers.get_settings = () => ({ ...SETTINGS, filePrefix: "Newer" });
    fireEvent.focus(window);
    await flush();
    expect(screen.getByText(/Mine_YYYY-MM-DD/)).toBeTruthy();
  });

  it("explains missing permissions and offers to request them", async () => {
    tauri.handlers.platform_info = () => platform({ screenPermission: false });
    tauri.handlers.request_screen_permission = () => true;
    await mountLoaded();
    await screen.findByText("Screen Recording permission");
    tauri.handlers.platform_info = () => platform();
    fireEvent.click(screen.getByText("Request access"));
    await waitFor(() => expect(screen.queryByText("Screen Recording permission")).toBeNull());
    expect(tauri.calls("request_screen_permission")).toHaveLength(1);
  });

  it("opens the system settings for the permission", async () => {
    tauri.handlers.platform_info = () => platform({ screenPermission: false });
    await mountLoaded();
    fireEvent.click(await screen.findByText("Open System Settings"));
    expect(tauri.calls("open_screen_permission_settings")).toHaveLength(1);
  });

  it("warns about translocated or disk-image installs instead of the permission", async () => {
    tauri.handlers.platform_info = () => platform({ screenPermission: false, installIssue: "translocated" });
    const { unmount } = await mountLoaded();
    await screen.findByText("Move Socorin to Applications");
    expect(screen.getByText(/relocated copy/)).toBeTruthy();
    expect(screen.queryByText("Screen Recording permission")).toBeNull();
    unmount();

    tauri.handlers.platform_info = () => platform({ installIssue: "disk-image" });
    await mountLoaded();
    await screen.findByText(/running from the disk image/);
  });

  it("warns about Wayland", async () => {
    tauri.handlers.platform_info = () => platform({ os: "linux", wayland: true });
    await mountLoaded();
    await screen.findByText("Wayland session detected");
    // The Wayland card and the System hint both name the binary to bind.
    expect(screen.getAllByText("socorin --capture")).toHaveLength(2);
  });

  it("describes the built-in recorder on Windows", async () => {
    tauri.handlers.platform_info = () => platform({ os: "windows" });
    await mountLoaded();
    await screen.findByText(/record with the built-in recorder/);
    expect(screen.getByPlaceholderText("Built-in recorder")).toBeTruthy();
    expect(screen.getByText("ffmpeg.exe")).toBeTruthy();
    expect(screen.queryByText(/Screen recording uses ffmpeg/)).toBeNull();
    expect(screen.getByText(/read through WASAPI/)).toBeTruthy();
    expect(screen.queryByText(/asks for microphone access/)).toBeNull();
  });

  it("lists the microphones, keeps an unplugged choice and saves the switch", async () => {
    const { container } = await mountLoaded();
    const micSwitch = screen.getByLabelText("Record the microphone") as HTMLInputElement;
    expect(micSwitch.checked).toBe(true);
    const select = () => container.querySelector("select") as HTMLSelectElement;
    await waitFor(() => expect(select().options).toHaveLength(3));
    expect(Array.from(select().options).map((o) => o.textContent)).toEqual([
      "System default (MacBook Pro Microphone)",
      "MacBook Pro Microphone",
      "Jabra Speak 710",
    ]);
    expect(select().value).toBe("");
    expect(select().disabled).toBe(false);
    expect(screen.getByText(/asks for microphone access the first time/)).toBeTruthy();
    expect(screen.queryByText(/No microphone is connected/)).toBeNull();
    expect(screen.queryByText(/Microphone access is off/)).toBeNull();

    // Choosing one remembers its name too, so it can be shown while unplugged.
    fireEvent.change(select(), { target: { value: "usb:1" } });
    vi.useFakeTimers();
    fireEvent.click(saveButton());
    await act(() => vi.advanceTimersByTimeAsync(1));
    const patch = (tauri.calls("update_settings")[0] as { patch: Record<string, unknown> }).patch;
    expect(patch).toMatchObject({ mic: true, micDevice: "usb:1", micDeviceName: "Jabra Speak 710" });
    vi.useRealTimers();

    // Off: the list is greyed out.
    fireEvent.click(micSwitch);
    expect(micSwitch.checked).toBe(false);
    expect(select().disabled).toBe(true);
    fireEvent.click(micSwitch);
    expect(select().disabled).toBe(false);

    // The list is refreshed when the window comes back (a headset plugged in).
    tauri.handlers.audio_inputs = () => [];
    act(() => window.dispatchEvent(new Event("focus")));
    await screen.findByText(/No microphone is connected/);
    expect(select().options[0].textContent).toBe("System default");
  });

  it("keeps a chosen microphone that is not connected, and points at the permission when it is off", async () => {
    tauri.handlers.get_settings = () => ({ ...SETTINGS, micDevice: "gone", micDeviceName: "Old Headset" });
    tauri.handlers.platform_info = () => platform({ micPermission: "denied" });
    const { container } = await mountLoaded();
    const select = container.querySelector("select") as HTMLSelectElement;
    await waitFor(() => expect(select.options).toHaveLength(4));
    expect(select.value).toBe("gone");
    expect(select.options[3].textContent).toBe("Old Headset (not connected)");
    await screen.findByText(/Microphone access is off for Socorin/);
    fireEvent.click(screen.getByText("Open System Settings"));
    expect(tauri.calls("open_mic_permission_settings")).toHaveLength(1);
    // Back to the system default: no name to remember.
    fireEvent.change(select, { target: { value: "" } });
    vi.useFakeTimers();
    fireEvent.click(saveButton());
    await act(() => vi.advanceTimersByTimeAsync(1));
    const patch = (tauri.calls("update_settings")[0] as { patch: Record<string, unknown> }).patch;
    expect(patch).toMatchObject({ micDevice: "", micDeviceName: "" });
  });

  it("describes ffmpeg on Linux and the MP4 conversion on macOS", async () => {
    tauri.handlers.platform_info = () => platform({ os: "linux" });
    const { unmount } = await mountLoaded();
    await screen.findByText(/Screen recording uses ffmpeg/);
    expect(screen.getByPlaceholderText("Found automatically")).toBeTruthy();
    expect(screen.getByText(/listed with pactl/)).toBeTruthy();
    unmount();

    tauri.handlers.platform_info = () => platform();
    await mountLoaded();
    await screen.findByText(/converts it with ffmpeg/);
    expect(screen.getByPlaceholderText("Found automatically (Homebrew)")).toBeTruthy();
    expect(screen.queryByText(/Screen recording uses ffmpeg/)).toBeNull();
  });

  it("saves the upload server, offers the upload mode and reports a link", async () => {
    await mountLoaded();
    vi.useFakeTimers();
    fireEvent.click(screen.getByLabelText("Upload and copy the link immediately"));
    fireEvent.change(screen.getByPlaceholderText("https://socorin.com"), { target: { value: "http://localhost:3000" } });
    fireEvent.click(saveButton());
    await act(() => vi.advanceTimersByTimeAsync(1));
    const patch = (tauri.calls("update_settings")[0] as { patch: Record<string, unknown> }).patch;
    expect(patch).toMatchObject({ afterCapture: "upload", uploadServer: "http://localhost:3000" });
    expect(patch).not.toHaveProperty("installId");
    act(() => tauri.emit("capture-done", "link"));
    expect(screen.getByText("Link copied").className).toContain("ok");
  });

  it("states what socorin.com keeps: 60 days, 15 MB per file", async () => {
    const { container } = await mountLoaded();
    const hint = [...container.querySelectorAll("p.hint")].find((p) => p.textContent?.includes("Upload & copy link"));
    expect(hint?.textContent).toMatch(/\(60 days and\s+15 MB per file on socorin\.com\)/);
  });

  it("shows the install id with a reset, and the shared links with copy and delete", async () => {
    tauri.handlers.get_settings = () => ({ ...SETTINGS, installId: "inst-00000000000000001" });
    tauri.handlers.share_history = () => [LINK];
    tauri.handlers.reset_install_id = () => ({ ...SETTINGS, installId: "" });
    const { container } = await mountLoaded();
    await screen.findByText(LINK.shareUrl);
    expect(screen.getByText(/^image · 1\.2 MB · expires .*2026$/)).toBeTruthy(); // locale-dependent date
    expect((container.querySelector("input[value='inst-00000000000000001']") as HTMLInputElement).readOnly).toBe(true);
    const reset = screen.getByText("Reset install ID") as HTMLButtonElement;
    expect(reset.disabled).toBe(false);
    fireEvent.click(reset);
    await screen.findByText(/Install ID reset/);
    expect(tauri.calls("reset_install_id")).toHaveLength(1);
    expect((screen.getByText("Reset install ID") as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByPlaceholderText("Not registered yet")).toBeTruthy();

    fireEvent.click(screen.getByText("Copy"));
    await screen.findByText("Link copied");
    expect(tauri.calls("copy_share_link")[0]).toEqual({ id: LINK.id });
    tauri.handlers.copy_share_link = () => Promise.reject("This link is no longer in the list.");
    fireEvent.click(screen.getByText("Copy"));
    await screen.findByText("This link is no longer in the list.");

    // A refused delete keeps the line and shows the server's sentence.
    tauri.handlers.delete_share = () => Promise.reject({ code: "forbidden", message: "socorin.com refused this upload." });
    fireEvent.click(screen.getByText("Delete"));
    await screen.findByText("socorin.com refused this upload.");
    expect(screen.getByText(LINK.shareUrl)).toBeTruthy();
    tauri.handlers.delete_share = () => undefined;
    fireEvent.click(screen.getByText("Delete"));
    await screen.findByText("Deleted from server");
    expect(tauri.calls("delete_share")).toEqual([{ id: LINK.id }, { id: LINK.id }]);
    expect(screen.queryByText(LINK.shareUrl)).toBeNull();
    expect(screen.getByText(/No links yet/)).toBeTruthy();

    // Rust says the history changed (another window uploaded): reloaded.
    tauri.handlers.share_history = () => [{ ...LINK, id: "other", shareUrl: "https://socorin.com/s/other", kind: "video", size: 20_000, expiresAt: "nope" }];
    act(() => tauri.emit("share:history"));
    await screen.findByText("https://socorin.com/s/other");
    expect(screen.getByText("video · 20 KB")).toBeTruthy();
    expect(describeLink({ ...LINK, size: 100 })).toMatch(/^image · 1 KB · expires .*2026$/);

    tauri.handlers.reset_install_id = () => Promise.reject("disk full");
    tauri.handlers.get_settings = () => ({ ...SETTINGS, installId: "inst-2" });
    fireEvent.focus(window);
    await screen.findByText("inst-2", { selector: "input" }).catch(() => {});
    fireEvent.click(await screen.findByText("Reset install ID"));
    await screen.findByText("disk full");
  });

  it("wires the footer actions", async () => {
    await mountLoaded();
    fireEvent.click(screen.getByText("Capture now"));
    fireEvent.click(screen.getByRole("button", { name: "Record region" }));
    fireEvent.click(screen.getByText("Open screenshots folder"));
    fireEvent.click(screen.getByText("Quit Socorin"));
    expect(tauri.calls("start_capture")).toHaveLength(1);
    expect(tauri.calls("start_record_region")).toHaveLength(1);
    expect(tauri.calls("open_save_dir")).toHaveLength(1);
    expect(tauri.calls("quit")).toHaveLength(1);
    const link = screen.getByText("socorin.com");
    expect(fireEvent.click(link)).toBe(false);
    expect(tauri.calls("plugin:opener|open_url")[0]).toMatchObject({ url: "https://socorin.com" });
    expect(screen.getByText(new RegExp(`© ${new Date().getFullYear()} Socorin`))).toBeTruthy();
    expect(screen.getByText(new RegExp(isMac ? "menu bar" : "system tray"))).toBeTruthy();
  });

  it("shows the load error when the settings cannot be read", async () => {
    tauri.handlers.get_settings = () => Promise.reject("io error");
    render(<Settings />);
    await flush();
    expect(screen.getByText("Loading…")).toBeTruthy();
  });

  it("describes the update state and checks on demand", async () => {
    await mountLoaded();
    await screen.findByText(/Socorin 1\.0\.2\. Not checked yet\./);
    expect((screen.getByLabelText("Check for updates automatically (once a day)") as HTMLInputElement).checked).toBe(true);
    const auto = screen.getByLabelText("Install updates automatically") as HTMLInputElement;
    expect(auto.checked).toBe(true);
    expect(auto.disabled).toBe(false);
    // Automatic installs need the daily check.
    fireEvent.click(screen.getByLabelText("Check for updates automatically (once a day)"));
    expect(auto.disabled).toBe(true);
    expect(screen.queryByText(/Update to/)).toBeNull();

    // Check now: nothing newer.
    const checkedAt = Math.floor(Date.now() / 1000);
    tauri.handlers.check_for_updates = () => ({ ...UPDATE_STATUS, checkedAt });
    fireEvent.click(screen.getByText("Check now"));
    expect(screen.getByText("Checking…")).toBeTruthy();
    await screen.findByText("Socorin is up to date");
    expect(screen.getByText(/Up to date, last checked today/)).toBeTruthy();

    // Check now: a new version, with its Update button.
    tauri.handlers.check_for_updates = () => ({ ...UPDATE_STATUS, available: "1.1.0", checkedAt });
    fireEvent.click(screen.getByText("Check now"));
    await screen.findByText("Socorin 1.1.0 is available");
    expect(screen.getByText(/Version 1\.1\.0 is available\./)).toBeTruthy();
    fireEvent.click(screen.getByText("Update to 1.1.0"));
    expect(tauri.calls("install_update")).toHaveLength(1);

    // Progress arrives by event; the buttons wait.
    act(() => tauri.emit("update:status", { ...UPDATE_STATUS, available: "1.1.0", phase: { phase: "installing", received: 1, total: 2 } }));
    expect((screen.getByText("Updating…") as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByText("Check now") as HTMLButtonElement).disabled).toBe(true);
    act(() => tauri.emit("update:status", { ...UPDATE_STATUS, available: "", checkedAt: 1_700_000_000 }));
    expect(screen.getByText(/last checked .*2023\./)).toBeTruthy(); // locale-dependent date

    // A failed check is reported in the status line.
    tauri.handlers.check_for_updates = () => Promise.reject("Cannot reach socorin.com");
    fireEvent.click(screen.getByText("Check now"));
    await screen.findByText("Cannot reach socorin.com");
    expect(formatChecked(0, new Date(0))).toMatch(/^today /);
  });

  it("copes without an update status", async () => {
    tauri.handlers.update_status = () => Promise.reject("nope");
    await mountLoaded();
    await flush();
    expect(screen.getByText(/^Socorin \.\s*$/)).toBeTruthy();
    expect(screen.getByText("Check now")).toBeTruthy();
  });
});
