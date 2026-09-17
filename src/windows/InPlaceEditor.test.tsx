import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { isMac } from "../lib/hotkey";
import { flush, installTauri, SETTINGS, type TauriMock } from "../test/tauri";
import { InPlaceEditor, type Crop } from "./InPlaceEditor";

const mod = isMac ? { metaKey: true } : { ctrlKey: true };
let tauri: TauriMock;

function source(w = 2048, h = 1536) {
  const c = document.createElement("canvas");
  c.width = w;
  c.height = h;
  return c;
}

function mount(props: Partial<Parameters<typeof InPlaceEditor>[0]> = {}) {
  const onLockChange = vi.fn();
  const all = {
    source: source(),
    crop: { x: 200, y: 200, width: 400, height: 300 } as Crop,
    scale: 0.5,
    hidden: false,
    onLockChange,
    settings: SETTINGS,
    ...props,
  };
  const view = render(<InPlaceEditor {...all} />);
  return { ...view, onLockChange, props: all };
}

beforeEach(() => {
  tauri = installTauri("overlay-1", {
    debug_options: () => ({ enabled: false, dumpDir: null, autoSelect: null, autoAction: null }),
    copy_png: () => undefined,
    save_png: () => "/tmp/shots/x.png",
  });
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 600 });
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 40 });
});

describe("InPlaceEditor", () => {
  it("places the stage over the selection and the toolbar below it", () => {
    const { container, onLockChange } = mount();
    const stage = container.querySelector(".inplace-stage") as HTMLElement;
    expect(stage.style.left).toBe("100px");
    expect(stage.style.top).toBe("100px");
    expect(stage.style.width).toBe("200px");
    expect(stage.style.height).toBe("150px");
    expect(stage.style.pointerEvents).toBe("none");
    const toolbar = container.querySelector(".floating-toolbar") as HTMLElement;
    expect(toolbar.style.top).toBe(`${100 + 150 + 8}px`);
    expect(toolbar.style.left).toBe("8px"); // right-aligned but clamped to the left gap
    expect(onLockChange).toHaveBeenLastCalledWith(false);
  });

  it("moves the toolbar above or inside the selection when there is no room below", () => {
    const { container, rerender, props } = mount({ crop: { x: 0, y: 1400, width: 1000, height: 130 } });
    let toolbar = container.querySelector(".floating-toolbar") as HTMLElement;
    expect(toolbar.style.top).toBe(`${700 - 8 - 40}px`);
    rerender(<InPlaceEditor {...props} crop={{ x: 0, y: 0, width: 2048, height: 1536 }} />);
    toolbar = container.querySelector(".floating-toolbar") as HTMLElement;
    expect(toolbar.style.top).toBe(`${Math.max(8, 768 - 8 - 40)}px`);
    expect(toolbar.style.left).toBe(`${1024 - 600 - 8}px`);
  });

  it("locks the selection once a tool is active and unlocks when it is toggled off", () => {
    const { container, onLockChange } = mount();
    fireEvent.click(screen.getByTitle("Rectangle (R)"));
    expect(onLockChange).toHaveBeenLastCalledWith(true);
    expect((container.querySelector(".inplace-stage") as HTMLElement).style.pointerEvents).toBe("auto");
    expect(container.querySelectorAll(".toolbar-row")).toHaveLength(2);
    fireEvent.click(screen.getByTitle("Rectangle (R)"));
    expect(onLockChange).toHaveBeenLastCalledWith(false);
  });

  it("copies and finishes, saves and finishes, saves as, opens the editor and closes", async () => {
    mount();
    fireEvent.click(screen.getByTitle("Copy to clipboard and finish (Enter, Ctrl/⌘+C)"));
    await waitFor(() => expect(tauri.calls("cancel_capture")).toHaveLength(1));
    expect(tauri.calls("copy_png")[0]).toBeInstanceOf(Uint8Array);

    fireEvent.click(screen.getByTitle("Save to screenshots folder (Ctrl/⌘+S)"));
    await waitFor(() => expect(tauri.calls("cancel_capture")).toHaveLength(2));
    expect(tauri.calls("save_png")).toHaveLength(1);

    fireEvent.click(screen.getByTitle("Save as… (Ctrl/⌘+Shift+S)"));
    await waitFor(() => expect(tauri.calls("save_png_as")).toHaveLength(1));

    fireEvent.click(screen.getByTitle("Open in the editor window (Ctrl/⌘+E)"));
    await waitFor(() => expect(tauri.calls("edit_png")).toHaveLength(1));

    fireEvent.click(screen.getByTitle("Close (Esc)"));
    expect(tauri.calls("cancel_capture")).toHaveLength(3);
  });

  it("supports the keyboard and the double-click-to-finish gesture", async () => {
    mount();
    fireEvent.keyDown(window, { key: "Enter" });
    await waitFor(() => expect(tauri.calls("copy_png")).toHaveLength(1));
    fireEvent.keyDown(window, { key: "c", ...mod });
    await waitFor(() => expect(tauri.calls("copy_png")).toHaveLength(2));
    fireEvent.keyDown(window, { key: "s", ...mod });
    await waitFor(() => expect(tauri.calls("save_png")).toHaveLength(1));
    fireEvent.keyDown(window, { key: "S", shiftKey: true, ...mod });
    await waitFor(() => expect(tauri.calls("save_png_as")).toHaveLength(1));
    fireEvent.keyDown(window, { key: "e", ...mod });
    await waitFor(() => expect(tauri.calls("edit_png")).toHaveLength(1));
    fireEvent.keyDown(window, { key: "Escape" });
    expect(tauri.calls("cancel_capture").length).toBeGreaterThanOrEqual(4);

    const copies = tauri.calls("copy_png").length;
    fireEvent.dblClick(window, { clientX: 50, clientY: 50, button: 0 });
    await flush();
    expect(tauri.calls("copy_png")).toHaveLength(copies);
    fireEvent.dblClick(window, { clientX: 150, clientY: 150, button: 0 });
    await waitFor(() => expect(tauri.calls("copy_png")).toHaveLength(copies + 1));

    // With a tool active the double click belongs to the stage.
    fireEvent.keyDown(window, { key: "r" });
    fireEvent.dblClick(window, { clientX: 150, clientY: 150, button: 0 });
    await flush();
    expect(tauri.calls("copy_png")).toHaveLength(copies + 1);
  });

  it("uploads, says so meanwhile, and ends the capture once the link is copied", async () => {
    let finish: (v: unknown) => void = () => {};
    tauri.handlers.upload_png = () => new Promise((r) => (finish = r));
    const { container } = mount();
    fireEvent.click(screen.getByTitle("Upload & copy link (Ctrl/⌘+Shift+U)"));
    await waitFor(() => expect(tauri.calls("upload_png")).toHaveLength(1));
    expect(tauri.calls("upload_png")[0]).toBeInstanceOf(Uint8Array);
    const toast = container.querySelector(".inplace-toast") as HTMLElement;
    expect(toast.textContent).toBe("Uploading…");
    expect(toast.className).toContain("info");
    expect((screen.getByTitle("Upload & copy link (Ctrl/⌘+Shift+U)") as HTMLButtonElement).disabled).toBe(true);
    expect(tauri.calls("cancel_capture")).toHaveLength(0);
    finish({ id: "x", shareUrl: "https://socorin.com/s/x" });
    await waitFor(() => expect(tauri.calls("cancel_capture")).toHaveLength(1));
    expect(container.querySelector(".inplace-toast")).toBeNull();

    // The keyboard shortcut does the same.
    fireEvent.keyDown(window, { key: "U", shiftKey: true, ...mod });
    await waitFor(() => expect(tauri.calls("upload_png")).toHaveLength(2));
    finish({ id: "y", shareUrl: "https://socorin.com/s/y" });
    await waitFor(() => expect(tauri.calls("cancel_capture")).toHaveLength(2));
  });

  it("shows the share error's own sentence and keeps the capture open", async () => {
    tauri.handlers.upload_png = () => Promise.reject({ code: "file_too_large", message: "This capture is 7.3 MB; the share limit is 5 MB.", maxBytes: 5_242_880 });
    const { container } = mount();
    fireEvent.click(screen.getByTitle("Upload & copy link (Ctrl/⌘+Shift+U)"));
    await waitFor(() => expect(container.querySelector(".inplace-toast")?.textContent).toBe("This capture is 7.3 MB; the share limit is 5 MB."));
    expect(container.querySelector(".inplace-toast")?.className).not.toContain("info");
    expect(tauri.calls("cancel_capture")).toHaveLength(0);
  });

  it("shows a toast when an action fails and hides it later", async () => {
    vi.useFakeTimers();
    tauri.handlers.copy_png = () => Promise.reject("nope");
    const { container } = mount();
    fireEvent.click(screen.getByTitle("Copy to clipboard and finish (Enter, Ctrl/⌘+C)"));
    await act(() => vi.advanceTimersByTimeAsync(1));
    expect(container.querySelector(".inplace-toast")?.textContent).toBe("nope");
    await act(() => vi.advanceTimersByTimeAsync(6000));
    expect(container.querySelector(".inplace-toast")).toBeNull();
  });

  it("hides itself while the selection is being adjusted", () => {
    const { container } = mount({ hidden: true });
    expect((container.querySelector(".inplace") as HTMLElement).style.visibility).toBe("hidden");
  });

  it("runs the automated end-to-end action", async () => {
    vi.useFakeTimers();
    tauri.handlers.debug_options = () => ({ enabled: true, dumpDir: null, autoSelect: null, autoAction: "copy" });
    mount({ settings: null });
    await act(() => vi.advanceTimersByTimeAsync(1));
    expect((screen.getByTitle("Undo (Ctrl/⌘+Z)") as HTMLButtonElement).disabled).toBe(false);
    await act(() => vi.advanceTimersByTimeAsync(810));
    expect(tauri.calls("copy_png")).toHaveLength(1);
  });

  it("saves for the save auto action", async () => {
    vi.useFakeTimers();
    tauri.handlers.debug_options = () => ({ enabled: true, dumpDir: null, autoSelect: null, autoAction: "save" });
    mount();
    await act(() => vi.advanceTimersByTimeAsync(820));
    expect(tauri.calls("save_png")).toHaveLength(1);
  });
});
