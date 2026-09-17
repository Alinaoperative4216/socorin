import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { MonitorInfo } from "../lib/ipc";
import { flush, installTauri, SETTINGS, type TauriMock } from "../test/tauri";
import { Overlay } from "./Overlay";

let tauri: TauriMock;

const info = (extra: Partial<MonitorInfo> = {}): MonitorInfo => ({
  id: 1,
  name: "Main",
  x: 0,
  y: 0,
  width: 1024,
  height: 768,
  scale: 2,
  primary: true,
  imageWidth: 2048,
  imageHeight: 1536,
  session: 1,
  preselectFull: false,
  mode: "screenshot",
  ...extra,
});

const HINT = /Drag to select an area/;

beforeEach(() => {
  tauri = installTauri("overlay-1", {
    overlay_info: () => Promise.reject("no capture session"),
    overlay_pixels: () => new ArrayBuffer(2048 * 1536 * 4),
    get_settings: () => SETTINGS,
    debug_options: () => ({ enabled: false, dumpDir: null, autoSelect: null, autoAction: null }),
    copy_png: () => undefined,
    save_png: () => "/tmp/x.png",
  });
  // Paint on the next microtask instead of a real frame so tests stay deterministic.
  let frame = 0;
  window.requestAnimationFrame = (cb) => {
    frame += 1;
    queueMicrotask(() => cb(performance.now()));
    return frame;
  };
  window.cancelAnimationFrame = () => {};
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 600 });
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 40 });
});

const mouse = (type: "mouseDown" | "mouseMove" | "mouseUp", x: number, y: number, button = 0) =>
  fireEvent[type](window, { clientX: x, clientY: y, button });

async function startSession(extra: Partial<MonitorInfo> = {}) {
  const view = render(<Overlay monitorId={1} />);
  await flush();
  act(() => tauri.emit("capture:start", info(extra)));
  await waitFor(() => expect(tauri.calls("overlay_ready")).toHaveLength(1));
  await act(() => flush()); // let React commit `ready`
  return view;
}

function ctxOf(container: HTMLElement) {
  const canvas = container.querySelector("canvas.overlay-canvas") as HTMLCanvasElement;
  return canvas.getContext("2d") as unknown as Record<string, ReturnType<typeof vi.fn>>;
}

async function drag(x1: number, y1: number, x2: number, y2: number) {
  mouse("mouseDown", x1, y1);
  mouse("mouseMove", x2, y2);
  mouse("mouseUp", x2, y2);
  await flush();
}

describe("Overlay: session lifecycle", () => {
  it("stays idle until Rust starts a session, then paints and reports ready", async () => {
    const { container } = render(<Overlay monitorId={1} />);
    await flush();
    expect(tauri.listeners().map((l) => l.event).sort()).toEqual(["capture:lock", "capture:reset", "capture:start", "cursor:move"]);
    expect(tauri.listeners()[0].target).toEqual({ kind: "WebviewWindow", label: "overlay-1" });
    expect(screen.queryByText(HINT)).toBeNull();

    act(() => tauri.emit("capture:start", info()));
    await screen.findByText(HINT);
    expect(tauri.calls("overlay_pixels")).toEqual([{ monitorId: 1 }]);
    expect(tauri.calls("overlay_ready")).toEqual([{ monitorId: 1 }]);
    const canvas = container.querySelector("canvas.overlay-canvas") as HTMLCanvasElement;
    expect(canvas.width).toBe(1024);
    expect(ctxOf(container).drawImage).toHaveBeenCalled();

    // Duplicate signal for the same session and signals for other monitors are ignored.
    act(() => tauri.emit("capture:start", info()));
    act(() => tauri.emit("capture:start", info({ id: 2, session: 5 })));
    await flush();
    expect(tauri.calls("overlay_pixels")).toHaveLength(1);

    act(() => tauri.emit("capture:reset"));
    expect(screen.queryByText(HINT)).toBeNull();
    expect(canvas.width).toBe(0);
  });

  it("picks up a session that was already running when the window loaded", async () => {
    tauri.handlers.overlay_info = () => info({ session: 3 });
    render(<Overlay monitorId={1} />);
    await screen.findByText(HINT);
  });

  it("drops a start that was superseded while its pixels were loading", async () => {
    const release: ((b: ArrayBuffer) => void)[] = [];
    tauri.handlers.overlay_pixels = () => new Promise<ArrayBuffer>((r) => release.push(r));
    render(<Overlay monitorId={1} />);
    await flush();
    act(() => tauri.emit("capture:start", info({ session: 1 })));
    await flush();
    act(() => tauri.emit("capture:start", info({ session: 2 })));
    await flush();
    release[0](new ArrayBuffer(2048 * 1536 * 4));
    await flush();
    expect(tauri.calls("overlay_ready")).toHaveLength(0);
    release[1](new ArrayBuffer(2048 * 1536 * 4));
    await waitFor(() => expect(tauri.calls("overlay_ready")).toHaveLength(1));
  });

  it("shows the error and cancels when the bitmap cannot be fetched", async () => {
    vi.useFakeTimers();
    vi.spyOn(console, "error").mockImplementation(() => {});
    tauri.handlers.overlay_pixels = () => Promise.reject("pixels gone");
    render(<Overlay monitorId={1} />);
    await act(() => vi.advanceTimersByTimeAsync(1));
    act(() => tauri.emit("capture:start", info()));
    await act(() => vi.advanceTimersByTimeAsync(1));
    expect(screen.getByText("pixels gone").className).toContain("overlay-error");
    await act(() => vi.advanceTimersByTimeAsync(1500));
    expect(tauri.calls("cancel_capture")).toHaveLength(1);
  });

  it("falls back to annotating when the settings cannot be read", async () => {
    tauri.handlers.get_settings = () => Promise.reject("nope");
    await startSession();
    await drag(100, 100, 300, 250);
    expect(tauri.calls("begin_annotation")).toEqual([{ monitorId: 1 }]);
  });
});

describe("Overlay: selecting", () => {
  it("drags a region and hands it to the in-place editor", async () => {
    const { container } = await startSession();
    mouse("mouseMove", 50, 60);
    await flush();
    expect(ctxOf(container).moveTo).toHaveBeenCalled(); // crosshair
    mouse("mouseDown", 100, 100);
    expect(screen.queryByText(HINT)).toBeNull();
    mouse("mouseMove", 300, 250);
    mouse("mouseUp", 300, 250);
    expect(tauri.calls("begin_annotation")).toEqual([{ monitorId: 1 }]);
    await screen.findByTitle("Close (Esc)");
    const stage = container.querySelector(".inplace-stage") as HTMLElement;
    expect(stage.style.left).toBe("100px");
    expect(stage.style.width).toBe("200px");
    expect(tauri.calls("finish_region")).toHaveLength(0);
    // The in-place editor owns Escape now.
    fireEvent.keyDown(window, { key: "Escape" });
    expect(tauri.calls("cancel_capture")).toHaveLength(1);
  });

  it("ignores tiny drags, other buttons and cancels on right click / Escape", async () => {
    await startSession();
    await drag(100, 100, 101, 101);
    expect(screen.getByText(HINT)).toBeTruthy();
    expect(tauri.calls("begin_annotation")).toHaveLength(0);
    mouse("mouseDown", 100, 100, 1);
    mouse("mouseUp", 100, 100, 1);
    expect(screen.getByText(HINT)).toBeTruthy();

    mouse("mouseDown", 10, 10, 2);
    expect(tauri.calls("cancel_capture")).toHaveLength(1);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(tauri.calls("cancel_capture")).toHaveLength(1); // already finished
  });

  it("cancels with Escape only once ready", async () => {
    render(<Overlay monitorId={1} />);
    await flush();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(tauri.calls("cancel_capture")).toHaveLength(0);
    act(() => tauri.emit("capture:start", info()));
    await screen.findByText(HINT);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(tauri.calls("cancel_capture")).toHaveLength(1);
  });

  it("reports the region straight to Rust in the immediate modes", async () => {
    tauri.handlers.get_settings = () => ({ ...SETTINGS, afterCapture: "clipboard" });
    await startSession();
    await drag(100, 100, 300, 250);
    expect(tauri.calls("finish_region")).toEqual([{ region: { monitorId: 1, x: 200, y: 200, width: 400, height: 300 } }]);
    expect(tauri.calls("begin_annotation")).toHaveLength(0);
    await drag(10, 10, 50, 50);
    expect(tauri.calls("finish_region")).toHaveLength(1); // finished: further drags ignored
  });

  it("shows the error when Rust rejects the region and allows retrying", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    tauri.handlers.get_settings = () => ({ ...SETTINGS, afterCapture: "save" });
    tauri.handlers.finish_region = () => Promise.reject("disk full");
    await startSession();
    await drag(100, 100, 300, 250);
    await screen.findByText("disk full");
    tauri.handlers.finish_region = () => undefined;
    await drag(100, 100, 300, 250);
    expect(tauri.calls("finish_region")).toHaveLength(2);
  });

  it("follows the native cursor events and the focus-follows-mouse rule", async () => {
    const { container } = await startSession();
    act(() => tauri.emit("cursor:move", { x: 1000, y: 750 }));
    await flush();
    expect(ctxOf(container).moveTo).toHaveBeenCalled();
    fireEvent.mouseEnter(document.documentElement);
    expect(tauri.calls("plugin:window|set_focus")).toHaveLength(1);
    expect(fireEvent.contextMenu(window)).toBe(false);
    fireEvent(window, new Event("resize"));
    await flush();
  });

  it("stays dimmed once another monitor's overlay owns the selection", async () => {
    await startSession();
    act(() => tauri.emit("capture:lock"));
    expect(screen.queryByText(HINT)).toBeNull();
    mouse("mouseDown", 100, 100);
    mouse("mouseMove", 200, 200);
    mouse("mouseUp", 200, 200);
    expect(tauri.calls("begin_annotation")).toHaveLength(0);
    fireEvent.mouseEnter(document.documentElement);
    expect(tauri.calls("plugin:window|set_focus")).toHaveLength(0);
    act(() => tauri.emit("cursor:move", { x: 10, y: 10 }));
    await flush();
  });
});

describe("Overlay: adjusting the selection", () => {
  async function annotate() {
    const view = await startSession();
    await drag(100, 100, 300, 250);
    await screen.findByTitle("Close (Esc)");
    const stage = () => view.container.querySelector(".inplace-stage") as HTMLElement;
    const root = () => view.container.querySelector(".overlay") as HTMLElement;
    return { ...view, stage, root };
  }

  it("resizes from a handle and moves from the inside, hiding the editor meanwhile", async () => {
    const { container, stage, root } = await annotate();
    mouse("mouseMove", 300, 250);
    expect(root().style.cursor).toBe("nwse-resize");
    mouse("mouseMove", 200, 100);
    expect(root().style.cursor).toBe("ns-resize");
    mouse("mouseMove", 200, 175);
    expect(root().style.cursor).toBe("move");
    mouse("mouseMove", 900, 700);
    expect(root().style.cursor).toBe("default");

    mouse("mouseDown", 300, 250);
    expect((container.querySelector(".inplace") as HTMLElement).style.visibility).toBe("hidden");
    mouse("mouseMove", 350, 300);
    mouse("mouseUp", 350, 300);
    await flush();
    expect((container.querySelector(".inplace") as HTMLElement).style.visibility).toBe("visible");
    expect(stage().style.width).toBe("250px");
    expect(stage().style.height).toBe("200px");

    mouse("mouseDown", 200, 200);
    mouse("mouseMove", 210, 190);
    mouse("mouseUp", 210, 190);
    await flush();
    expect(stage().style.left).toBe("110px");
    expect(stage().style.top).toBe("90px");

    // Dragging the north-west handle past the opposite edge flips the box.
    mouse("mouseDown", 110, 90);
    mouse("mouseMove", 500, 400);
    mouse("mouseUp", 500, 400);
    await flush();
    expect(stage().style.left).toBe("360px");
    expect(stage().style.width).toBe("140px");

    // Presses outside the selection do nothing.
    mouse("mouseDown", 1000, 700);
    mouse("mouseUp", 1000, 700);
    await flush();
    expect(stage().style.left).toBe("360px");
  });

  it("locks the selection once a tool is picked", async () => {
    const { stage, root } = await annotate();
    fireEvent.click(screen.getByTitle("Rectangle (R)"));
    expect(root().style.cursor).toBe("default");
    mouse("mouseMove", 300, 250);
    expect(root().style.cursor).toBe("default");
    mouse("mouseDown", 300, 250);
    mouse("mouseMove", 400, 400);
    mouse("mouseUp", 400, 400);
    await flush();
    expect(stage().style.width).toBe("200px");
  });
});

describe("Overlay: full screen, automation and recording", () => {
  it("selects the whole monitor at once for full-screen captures", async () => {
    await startSession({ preselectFull: true });
    await screen.findByTitle("Close (Esc)");
    expect(tauri.calls("begin_annotation")).toEqual([{ monitorId: 1 }]);
    expect(screen.queryByText(HINT)).toBeNull();
  });

  it("reports the whole monitor in the immediate modes", async () => {
    tauri.handlers.get_settings = () => ({ ...SETTINGS, afterCapture: "clipboard" });
    await startSession({ preselectFull: true });
    await waitFor(() =>
      expect(tauri.calls("finish_region")).toEqual([{ region: { monitorId: 1, x: 0, y: 0, width: 2048, height: 1536 } }]),
    );
  });

  it("auto-selects the debug region and logs diagnostics", async () => {
    vi.useFakeTimers();
    tauri.handlers.debug_options = () => ({ enabled: true, dumpDir: null, autoSelect: [10, 10, 100, 80], autoAction: null });
    render(<Overlay monitorId={1} />);
    await act(() => vi.advanceTimersByTimeAsync(1));
    act(() => tauri.emit("capture:start", info()));
    await act(() => vi.advanceTimersByTimeAsync(1));
    act(() => tauri.emit("cursor:move", { x: 5, y: 5 }));
    mouse("mouseMove", 6, 6);
    await act(() => vi.advanceTimersByTimeAsync(400));
    expect(tauri.calls("begin_annotation")).toEqual([{ monitorId: 1 }]);
    const logs = tauri.calls("debug_log").map((a) => (a as { message: string }).message);
    expect(logs.some((m) => m.includes("painted"))).toBe(true);
    expect(logs.some((m) => m.includes("first cursor:move"))).toBe(true);
    expect(logs.some((m) => m.includes("first DOM mousemove"))).toBe(true);
    expect(logs.some((m) => m.includes("annotate"))).toBe(true);
    act(() => tauri.emit("capture:reset"));
    expect(tauri.calls("debug_log").some((a) => (a as { message: string }).message.includes("reset"))).toBe(true);
    act(() => tauri.emit("capture:start", info({ session: 2 })));
    await act(() => vi.advanceTimersByTimeAsync(1));
    fireEvent.keyDown(window, { key: "Escape" });
    expect(tauri.calls("debug_log").some((a) => (a as { message: string }).message.includes("DOM Escape"))).toBe(true);
  });

  it("offers to record the selected area in record mode", async () => {
    tauri.handlers.debug_options = () => ({ enabled: true, dumpDir: null, autoSelect: null, autoAction: null });
    tauri.handlers.get_settings = () => ({ ...SETTINGS, afterCapture: "clipboard" });
    await startSession({ mode: "record" });
    expect(screen.getByText(/Drag to select the area to record/)).toBeTruthy();
    await drag(100, 100, 300, 250);
    expect(tauri.calls("begin_annotation")).toEqual([{ monitorId: 1 }]);
    expect(screen.queryByTitle("Close (Esc)")).toBeNull();
    fireEvent.click(screen.getByTitle("Start recording (Enter)"));
    // The bar's place goes along so the recording bar can take it over
    // (jsdom has no layout: zeros).
    expect(tauri.calls("start_recording")).toEqual([
      { region: { monitorId: 1, x: 200, y: 200, width: 400, height: 300 }, bar: { x: 0, y: 0, width: 0, height: 0 } },
    ]);
    fireEvent.click(screen.getByTitle("Cancel (Esc)"));
    expect(tauri.calls("cancel_capture")).toHaveLength(1);
  });

  it("shows why a recording could not start", async () => {
    tauri.handlers.start_recording = () => Promise.reject("ffmpeg missing");
    await startSession({ mode: "record" });
    await drag(100, 100, 300, 250);
    fireEvent.keyDown(window, { key: "Enter" });
    await screen.findByText("ffmpeg missing");
  });
});
