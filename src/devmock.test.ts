import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installMock } from "./devmock";

interface Internals {
  metadata: { currentWindow: { label: string }; currentWebview: { label: string; windowLabel: string } };
  invoke: (cmd: string, args?: unknown, options?: unknown) => Promise<unknown>;
  transformCallback: (cb: (...a: unknown[]) => void) => number;
  unregisterCallback: (id: number) => void;
  convertFileSrc: (path: string) => string;
}

const internals = () => (window as unknown as { __TAURI_INTERNALS__: Internals }).__TAURI_INTERNALS__;
let log: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  vi.useFakeTimers();
  log = vi.spyOn(console, "log").mockImplementation(() => {});
  window.history.replaceState({}, "", "/");
});

afterEach(() => {
  vi.clearAllTimers();
  window.history.replaceState({}, "", "/");
});

describe("devmock", () => {
  it("installs a runtime that identifies as the given window", () => {
    installMock("overlay-1");
    const i = internals();
    expect(i.metadata.currentWindow.label).toBe("overlay-1");
    expect(i.metadata.currentWebview).toEqual({ label: "overlay-1", windowLabel: "overlay-1" });
    expect(i.convertFileSrc("/a/b.png")).toBe("/a/b.png");
    expect(log).toHaveBeenCalledWith('[mock] Tauri runtime mocked as window "overlay-1"');

    const cb = vi.fn();
    const id = i.transformCallback(cb);
    expect((window as unknown as Record<string, unknown>)[`_${id}`]).toBe(cb);
    i.unregisterCallback(id);
    expect((window as unknown as Record<string, unknown>)[`_${id}`]).toBeUndefined();
  });

  it("describes a mock monitor and reads the mode from the query string", async () => {
    installMock("overlay-1");
    const idle = (await internals().invoke("overlay_info")) as Record<string, unknown>;
    expect(idle).toMatchObject({ id: 1, primary: true, session: 1, preselectFull: false, mode: "screenshot", width: 1024, height: 768 });
    window.history.replaceState({}, "", "/?mock=overlay-1&full=1&record=1");
    const full = (await internals().invoke("overlay_info")) as Record<string, unknown>;
    expect(full).toMatchObject({ preselectFull: true, mode: "record" });
  });

  it("serves synthetic pixels and a pending image", async () => {
    installMock("overlay-1");
    const pixels = (await internals().invoke("overlay_pixels")) as ArrayBuffer;
    expect(pixels.byteLength).toBe(1024 * 768 * 4);
    const png = (await internals().invoke("pending_image")) as ArrayBuffer;
    expect(png.byteLength).toBeGreaterThan(0);
  });

  it("keeps settings across updates and reports the platform from the query string", async () => {
    installMock("main");
    const s = (await internals().invoke("get_settings")) as { hotkey: string; filePrefix: string };
    expect(s.hotkey).toBe("CmdOrCtrl+Shift+A");
    const updated = (await internals().invoke("update_settings", { patch: { filePrefix: "X" } })) as { filePrefix: string };
    expect(updated.filePrefix).toBe("X");
    expect(((await internals().invoke("get_settings")) as { filePrefix: string }).filePrefix).toBe("X");

    expect(await internals().invoke("platform_info")).toEqual({ os: "macos", screenPermission: true, wayland: false, installIssue: null });
    window.history.replaceState({}, "", "/?mock=main&noperm=1&issue=disk-image");
    expect(await internals().invoke("platform_info")).toMatchObject({ screenPermission: false, installIssue: "disk-image" });
    expect(await internals().invoke("default_save_dir")).toBe("/Users/mock/Pictures/Screenshots");
    expect(await internals().invoke("debug_options")).toEqual({ enabled: false, dumpDir: null, autoSelect: null, autoAction: null });
  });

  it("logs the fire-and-forget commands", async () => {
    installMock("overlay-1");
    const buf = new ArrayBuffer(10);
    const calls: [string, unknown?, unknown?][] = [
      ["begin_annotation", { monitorId: 1 }],
      ["save_png_as", buf],
      ["edit_png", buf],
      ["overlay_ready", { monitorId: 1 }],
      ["debug_log", { message: "hi" }],
      ["welcome_ready"],
      ["dismiss_welcome"],
      ["start_record_region"],
      ["start_record_fullscreen"],
      ["start_recording", { region: {} }],
      ["stop_recording"],
      ["copy_png", buf],
      ["finish_region", { region: {} }],
      ["save_png", buf, { headers: {} }],
      ["save_png", new Uint8Array(3)],
      ["cancel_capture"],
      ["totally_unknown", { a: 1 }],
      ["another_unknown"],
    ];
    for (const [cmd, args, options] of calls) await internals().invoke(cmd, args, options);
    expect(await internals().invoke("save_png", buf)).toBe("/Users/mock/Pictures/Screenshots/mock.png");
    expect(await internals().invoke("totally_unknown")).toBeNull();
    expect(await internals().invoke("plugin:dialog|save")).toBe("/Users/mock/Pictures/Screenshots/save-as.png");
    expect(await internals().invoke("plugin:dialog|open")).toBe("/Users/mock/Pictures/Other");
    expect(await internals().invoke("plugin:event|listen")).toBe(1);
    const status = (await internals().invoke("recording_status")) as { recording: boolean };
    expect(status.recording).toBe(true);
    expect(log.mock.calls.length).toBeGreaterThan(calls.length);
  });
});
