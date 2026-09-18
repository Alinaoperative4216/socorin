import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { triggerResizeObservers } from "../test/setup";
import { flush, installTauri, SETTINGS, type TauriMock } from "../test/tauri";
import { RecordBar } from "./RecordBar";

let tauri: TauriMock;
const MICS = [
  { id: "builtin", name: "MacBook Pro Microphone", default: true },
  { id: "usb:1", name: "Jabra Speak 710", default: false },
];

beforeEach(() => {
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 300 });
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 40 });
  tauri = installTauri("overlay-1", { audio_inputs: () => MICS });
});

function mount(props: Partial<Parameters<typeof RecordBar>[0]> = {}) {
  const onStart = vi.fn();
  const onCancel = vi.fn();
  const onToggleMic = vi.fn();
  const view = render(
    <RecordBar
      crop={{ x: 200, y: 200, width: 400, height: 300 }}
      scale={0.5}
      hidden={false}
      settings={SETTINGS}
      onToggleMic={onToggleMic}
      onStart={onStart}
      onCancel={onCancel}
      {...props}
    />,
  );
  return { ...view, onStart, onCancel, onToggleMic, bar: () => view.container.querySelector(".floating-toolbar") as HTMLElement };
}

describe("RecordBar", () => {
  it("sits below the area, right-aligned and clamped to the window", () => {
    const { bar } = mount();
    expect(bar().style.top).toBe(`${100 + 150 + 8}px`);
    expect(bar().style.left).toBe("8px");
    act(() => triggerResizeObservers());
    expect(bar().style.left).toBe("8px");
  });

  it("goes above or inside when there is no room below", () => {
    const { bar, rerender, onStart, onCancel } = mount({ crop: { x: 800, y: 1400, width: 1000, height: 130 } });
    expect(bar().style.top).toBe(`${700 - 8 - 40}px`);
    expect(bar().style.left).toBe(`${Math.min(400 + 500 - 300, 1024 - 300 - 8)}px`);
    rerender(
      <RecordBar crop={{ x: 0, y: 0, width: 2048, height: 1536 }} scale={0.5} hidden settings={SETTINGS} onToggleMic={vi.fn()} onStart={onStart} onCancel={onCancel} />,
    );
    expect(bar().style.top).toBe(`${768 - 8 - 40}px`);
    expect((document.querySelector(".inplace") as HTMLElement).style.visibility).toBe("hidden");
  });

  it("starts with the button or Enter, reporting where the bar is, and cancels with the button or Escape", () => {
    const { onStart, onCancel, bar } = mount();
    // jsdom has no layout: the bar's rectangle comes from its computed place.
    bar().getBoundingClientRect = () => ({ left: 8, top: 258, width: 300, height: 40 }) as DOMRect;
    fireEvent.click(screen.getByTitle("Start recording (Enter)"));
    fireEvent.keyDown(window, { key: "Enter" });
    fireEvent.click(screen.getByTitle("Cancel (Esc)"));
    fireEvent.keyDown(window, { key: "Escape" });
    fireEvent.keyDown(window, { key: "x" });
    expect(onStart).toHaveBeenCalledTimes(2);
    expect(onStart).toHaveBeenLastCalledWith({ x: 8, y: 258, width: 300, height: 40 });
    expect(onCancel).toHaveBeenCalledTimes(2);
    expect(screen.getByText("Adjust the area, then")).toBeTruthy();
    expect(bar().querySelector(".toolbar-logo")).toBeTruthy();
  });

  it("names the microphone the recording will take and flips it with the button", async () => {
    const { onToggleMic, rerender, onStart, onCancel } = mount();
    // The list comes in after the bar is up; the button says so meanwhile.
    expect(screen.getByTitle("Microphone: system default — click to record without sound")).toBeTruthy();
    await screen.findByTitle("Microphone: MacBook Pro Microphone — click to record without sound");
    expect(tauri.calls("audio_inputs")).toHaveLength(1);
    fireEvent.click(screen.getByTitle(/Microphone: MacBook Pro Microphone/));
    expect(onToggleMic).toHaveBeenCalledTimes(1);
    // The switch is the owner's: the bar shows whatever the settings say.
    rerender(
      <RecordBar
        crop={{ x: 200, y: 200, width: 400, height: 300 }}
        scale={0.5}
        hidden={false}
        settings={{ ...SETTINGS, mic: false }}
        onToggleMic={onToggleMic}
        onStart={onStart}
        onCancel={onCancel}
      />,
    );
    expect(screen.getByTitle("Microphone off — click to record sound")).toBeTruthy();
    rerender(
      <RecordBar
        crop={{ x: 200, y: 200, width: 400, height: 300 }}
        scale={0.5}
        hidden={false}
        settings={{ ...SETTINGS, micDevice: "gone", micDeviceName: "Old Headset" }}
        onToggleMic={onToggleMic}
        onStart={onStart}
        onCancel={onCancel}
      />,
    );
    expect(screen.getByTitle(/Old Headset is not connected; recording with MacBook Pro Microphone/).className).toContain("issue");
  });

  it("copes with settings that did not load and a listing that fails", async () => {
    tauri.handlers.audio_inputs = () => Promise.reject("no audio");
    mount({ settings: null });
    await flush();
    // Rust records with the default in that case, and there is no list to
    // name it: the tooltip says so.
    expect(screen.getByTitle("No microphone is connected; recording without sound. — click to record without sound")).toBeTruthy();
  });
});
