import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { isMac } from "../lib/hotkey";
import { PNG_BYTES } from "../test/setup";
import { flush, installTauri, SETTINGS, type TauriMock } from "../test/tauri";
import { Editor } from "./Editor";

const mod = isMac ? { metaKey: true } : { ctrlKey: true };
let tauri: TauriMock;

beforeEach(() => {
  tauri = installTauri("editor", {
    pending_image: () => PNG_BYTES.buffer.slice(0),
    get_settings: () => SETTINGS,
    debug_options: () => ({ enabled: false, dumpDir: null, autoSelect: null, autoAction: null }),
    copy_png: () => undefined,
    save_png: () => "/tmp/shots/Socorin_1.png",
  });
  Object.defineProperty(HTMLElement.prototype, "clientWidth", { configurable: true, get: () => 500 });
  Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, get: () => 400 });
});

async function mountLoaded() {
  const view = render(<Editor />);
  expect(screen.getByText("Loading…")).toBeTruthy();
  await screen.findByText("800 × 600");
  await screen.findByText("58%"); // fitted to the 500×400 viewport
  return view;
}

describe("Editor window", () => {
  it("loads the pending image, sets the title and fits the zoom", async () => {
    await mountLoaded();
    expect(tauri.calls("plugin:window|set_title")[0]).toMatchObject({ value: "Socorin – 800 × 600" });
    expect(screen.getByText("Arrow")).toBeTruthy();
    fireEvent.click(screen.getByTitle("Fit to window"));
    expect(screen.getByText("58%")).toBeTruthy();
  });

  it("copies, saves and saves as through the toolbar", async () => {
    await mountLoaded();
    fireEvent.click(screen.getByTitle("Copy to clipboard (Ctrl/⌘+C)"));
    await screen.findByText("Copied to clipboard");
    expect(tauri.calls("copy_png")[0]).toBeInstanceOf(Uint8Array);

    fireEvent.click(screen.getByTitle("Save to screenshots folder (Ctrl/⌘+S)"));
    await screen.findByText("Saved to /tmp/shots/Socorin_1.png");
    expect(tauri.invoke).toHaveBeenCalledWith("save_png", expect.any(Uint8Array), undefined);

    // Save as… goes through Rust's own dialog: the page sends the PNG only,
    // never a path, and learns where it went from the reply.
    tauri.handlers.save_png_as = () => "/elsewhere/pic.png";
    fireEvent.click(screen.getByTitle("Save as… (Ctrl/⌘+Shift+S)"));
    await screen.findByText("Saved to /elsewhere/pic.png");
    expect(tauri.invoke).toHaveBeenLastCalledWith("save_png_as", expect.any(Uint8Array), undefined);
    expect(tauri.calls("plugin:dialog|save")).toHaveLength(0);
  });

  it("does nothing when the save dialog is cancelled and reports failures", async () => {
    await mountLoaded();
    tauri.handlers.save_png_as = () => null;
    fireEvent.click(screen.getByTitle("Save as… (Ctrl/⌘+Shift+S)"));
    await flush();
    expect(tauri.calls("save_png_as")).toHaveLength(1);
    expect(screen.queryByText(/Saved to/)).toBeNull();

    tauri.handlers.copy_png = () => Promise.reject("clipboard busy");
    fireEvent.click(screen.getByTitle("Copy to clipboard (Ctrl/⌘+C)"));
    const toast = await screen.findByText("clipboard busy");
    expect(toast.className).toContain("error");
  });

  it("handles the keyboard shortcuts", async () => {
    await mountLoaded();
    fireEvent.keyDown(window, { key: "c", ...mod });
    await screen.findByText("Copied to clipboard");
    fireEvent.keyDown(window, { key: "s", ...mod });
    await waitFor(() => expect(tauri.calls("save_png")).toHaveLength(1));
    tauri.handlers.save_png_as = () => null;
    fireEvent.keyDown(window, { key: "S", shiftKey: true, ...mod });
    await waitFor(() => expect(tauri.calls("save_png_as")).toHaveLength(1));

    // 58% sits between the 50% and 67% steps: zooming in lands on the step above 67%.
    fireEvent.keyDown(window, { key: "=", ...mod });
    expect(screen.getByText("75%")).toBeTruthy();
    fireEvent.keyDown(window, { key: "+", ...mod });
    expect(screen.getByText("100%")).toBeTruthy();
    fireEvent.keyDown(window, { key: "-", ...mod });
    expect(screen.getByText("75%")).toBeTruthy();
    fireEvent.keyDown(window, { key: "0", ...mod });
    expect(screen.getByText("58%")).toBeTruthy();

    fireEvent.keyDown(window, { key: "n" });
    expect(screen.getByText("next: 1")).toBeTruthy();
    expect(screen.getByText("Numbered marker")).toBeTruthy();

    fireEvent.keyDown(window, { key: "Escape" });
    expect(tauri.calls("plugin:window|close")).toHaveLength(1);
  });

  it("zooms with the modifier + wheel and clamps at the ends", async () => {
    const { container } = await mountLoaded();
    const viewport = container.querySelector(".canvas-viewport")!;
    fireEvent.wheel(viewport, { deltaY: -100 });
    expect(screen.getByText("58%")).toBeTruthy();
    fireEvent.wheel(viewport, { deltaY: -100, ...mod });
    expect(screen.getByText("75%")).toBeTruthy();
    for (let i = 0; i < 20; i++) fireEvent.wheel(viewport, { deltaY: 100, ...mod });
    expect(screen.getByText("10%")).toBeTruthy();
    for (let i = 0; i < 20; i++) fireEvent.wheel(viewport, { deltaY: -100, ...mod });
    expect(screen.getByText("400%")).toBeTruthy();
    fireEvent.click(screen.getByTitle("Zoom in"));
    expect(screen.getByText("400%")).toBeTruthy();
  });

  it("reloads when a new capture arrives", async () => {
    await mountLoaded();
    act(() => tauri.emit("editor:reload"));
    await waitFor(() => expect(tauri.calls("pending_image")).toHaveLength(2));
    await waitFor(() => expect(URL.revokeObjectURL).toHaveBeenCalled());
  });

  it("shows an empty state when there is nothing to edit", async () => {
    tauri.handlers.pending_image = () => Promise.reject("no image");
    render(<Editor />);
    await screen.findByText("No screenshot to edit yet.");
    fireEvent.click(screen.getByText("Capture a region"));
    expect(tauri.calls("start_capture")).toHaveLength(1);
  });

  it("shows the empty state when the image cannot be decoded", async () => {
    (URL.createObjectURL as ReturnType<typeof vi.fn>).mockReturnValueOnce("blob:fail");
    render(<Editor />);
    await screen.findByText("No screenshot to edit yet.");
  });

  it("runs the automated action from the debug options", async () => {
    vi.useFakeTimers();
    tauri.handlers.debug_options = () => ({ enabled: true, dumpDir: null, autoSelect: null, autoAction: "save" });
    render(<Editor />);
    await act(() => vi.advanceTimersByTimeAsync(10));
    expect(screen.getByText("800 × 600")).toBeTruthy();
    await act(() => vi.advanceTimersByTimeAsync(800));
    await act(() => vi.advanceTimersByTimeAsync(10));
    expect(tauri.calls("save_png")).toHaveLength(1);
    expect((screen.getByTitle("Undo (Ctrl/⌘+Z)") as HTMLButtonElement).disabled).toBe(false);
  });

  it("copies for the copy auto action", async () => {
    vi.useFakeTimers();
    tauri.handlers.debug_options = () => ({ enabled: true, dumpDir: null, autoSelect: null, autoAction: "copy" });
    render(<Editor />);
    await act(() => vi.advanceTimersByTimeAsync(10));
    await act(() => vi.advanceTimersByTimeAsync(810));
    expect(tauri.calls("copy_png")).toHaveLength(1);
  });
});
