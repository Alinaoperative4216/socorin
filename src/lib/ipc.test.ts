import { beforeEach, describe, expect, it, vi } from "vitest";
import { installTauri, type TauriMock } from "../test/tauri";
import { ipc, timestamp } from "./ipc";

let tauri: TauriMock;

beforeEach(() => {
  tauri = installTauri("main");
});

describe("ipc", () => {
  it("maps every wrapper to its command and arguments", async () => {
    const region = { monitorId: 1, x: 2, y: 3, width: 4, height: 5 };
    const bar = { x: 10, y: 20, width: 400, height: 42 };
    const cases: [() => Promise<unknown>, string, unknown?][] = [
      [ipc.debugOptions, "debug_options"],
      [() => ipc.debugLog("hi"), "debug_log", { message: "hi" }],
      [ipc.platformInfo, "platform_info"],
      [ipc.requestScreenPermission, "request_screen_permission"],
      [ipc.openScreenPermissionSettings, "open_screen_permission_settings"],
      [ipc.startCapture, "start_capture"],
      [ipc.startFullscreenCapture, "start_fullscreen_capture"],
      [ipc.cancelCapture, "cancel_capture"],
      [() => ipc.overlayInfo(7), "overlay_info", { monitorId: 7 }],
      [() => ipc.overlayPixels(7), "overlay_pixels", { monitorId: 7 }],
      [() => ipc.overlayReady(7), "overlay_ready", { monitorId: 7 }],
      [() => ipc.finishRegion(region), "finish_region", { region }],
      [() => ipc.beginAnnotation(7), "begin_annotation", { monitorId: 7 }],
      [ipc.endCapture, "cancel_capture"],
      [ipc.pendingImage, "pending_image"],
      [ipc.startRecordRegion, "start_record_region"],
      [ipc.startRecordFullscreen, "start_record_fullscreen"],
      [() => ipc.startRecording(region), "start_recording", { region, bar: null }],
      [() => ipc.startRecording(region, bar), "start_recording", { region, bar }],
      [ipc.stopRecording, "stop_recording"],
      [ipc.stopRecordingCopy, "stop_recording_copy"],
      [ipc.cancelRecording, "cancel_recording"],
      [ipc.recordingStatus, "recording_status"],
      [ipc.getSettings, "get_settings"],
      [() => ipc.updateSettings({ hotkey: "F5" }), "update_settings", { patch: { hotkey: "F5" } }],
      [ipc.welcomeReady, "welcome_ready"],
      [ipc.dismissWelcome, "dismiss_welcome"],
      [ipc.defaultSaveDir, "default_save_dir"],
      [ipc.openSaveDir, "open_save_dir"],
      [ipc.showSettings, "show_settings"],
      [ipc.quit, "quit"],
    ];
    for (const [call, cmd, args] of cases) {
      tauri.invoke.mockClear();
      await call();
      expect(tauri.invoke).toHaveBeenCalledTimes(1);
      const [gotCmd, gotArgs] = tauri.invoke.mock.calls[0];
      expect(gotCmd).toBe(cmd);
      if (args !== undefined) expect(gotArgs).toEqual(args);
    }
  });

  it("sends PNG bytes as the raw body", async () => {
    const png = new Uint8Array([1, 2, 3]);
    await ipc.copyPng(png);
    await ipc.editPng(png);
    await ipc.savePngAs(png);
    expect(tauri.invoke).toHaveBeenNthCalledWith(1, "copy_png", png, undefined);
    expect(tauri.invoke).toHaveBeenNthCalledWith(2, "edit_png", png, undefined);
    expect(tauri.invoke).toHaveBeenNthCalledWith(3, "save_png_as", png, undefined);
  });

  it("save_png returns the auto-named path and never sends one", async () => {
    tauri.handlers.save_png = () => "/out/a.png";
    const png = new Uint8Array([9]);
    expect(await ipc.savePng(png)).toBe("/out/a.png");
    expect(tauri.invoke).toHaveBeenLastCalledWith("save_png", png, undefined);
  });

  it("save_png_as resolves to the chosen path or null", async () => {
    const png = new Uint8Array([9]);
    tauri.handlers.save_png_as = () => "/Users/me/Ảnh/my shot.png";
    expect(await ipc.savePngAs(png)).toBe("/Users/me/Ảnh/my shot.png");
    tauri.handlers.save_png_as = () => null;
    expect(await ipc.savePngAs(png)).toBeNull();
  });

  it("propagates command errors", async () => {
    tauri.handlers.get_settings = () => Promise.reject("boom");
    await expect(ipc.getSettings()).rejects.toBe("boom");
  });
});

describe("timestamp", () => {
  it("formats the local time as YYYY-MM-DD_HH-mm-ss", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 8, 17, 9, 5, 7));
    expect(timestamp()).toBe("2026-09-17_09-05-07");
  });
});
