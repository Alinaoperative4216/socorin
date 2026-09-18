import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { installTauri, type TauriMock } from "../test/tauri";
import { RECORDER_MARGIN } from "../lib/ipc";
import { Recorder } from "./Recorder";

let tauri: TauriMock;

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(2026, 8, 17, 12, 0, 0));
  tauri = installTauri("recorder", {
    recording_status: () => ({ recording: true, startedMs: Date.now() - 65_000, path: "/tmp/a.mov", audio: "Jabra Speak 710", audioIssue: null }),
  });
});

describe("Recorder bar", () => {
  it("draws the bar inside the transparent margin and keeps counting", async () => {
    const { container } = render(<Recorder />);
    const bar = container.querySelector(".floating-toolbar") as HTMLElement;
    expect(bar.style.left).toBe(`${RECORDER_MARGIN}px`);
    expect(bar.style.top).toBe(`${RECORDER_MARGIN}px`);
    expect(screen.getByText("00:00")).toBeTruthy();
    await act(() => vi.advanceTimersByTimeAsync(1));
    expect(screen.getByText("01:05")).toBeTruthy();
    expect(container.querySelector(".rec-dot")?.className).toContain("live");
    await act(() => vi.advanceTimersByTimeAsync(2000));
    expect(screen.getByText("01:07")).toBeTruthy();
    // The microphone indicator says which one is recorded.
    const mic = screen.getByLabelText("Microphone");
    expect(mic.getAttribute("title")).toBe("Microphone: Jabra Speak 710");
    expect(mic.className).toContain("on");
  });

  it("follows reset / started / stopped and says when the file was copied", async () => {
    tauri.handlers.recording_status = () => ({ recording: false, startedMs: 0, path: "" });
    const { container } = render(<Recorder />);
    await act(() => vi.advanceTimersByTimeAsync(1));
    expect(screen.getByText("00:00")).toBeTruthy();
    expect(container.querySelector(".rec-dot")?.className).not.toContain("live");

    // Before the encoder is up the microphone is not known yet.
    expect(screen.getByLabelText("Microphone").className).toContain("unknown");
    act(() =>
      tauri.emit("recording:started", {
        recording: true,
        startedMs: Date.now() - 3_725_000,
        path: "/tmp/b.mov",
        audio: null,
        audioIssue: "No microphone is connected; recording without sound.",
      }),
    );
    await act(() => vi.advanceTimersByTimeAsync(250));
    expect(screen.getByText("1:02:05")).toBeTruthy();
    expect(screen.getByLabelText("Microphone").getAttribute("title")).toBe("Microphone off: No microphone is connected; recording without sound.");
    expect(screen.getByLabelText("Microphone").className).toContain("off issue");

    act(() => tauri.emit("recording:stopped", { copied: true }));
    expect(screen.getByText("Copied to clipboard")).toBeTruthy();
    act(() => tauri.emit("recording:reset"));
    expect(screen.getByText("00:00")).toBeTruthy();
    act(() => tauri.emit("recording:stopped", { copied: false }));
    expect(screen.getByText("00:00")).toBeTruthy();

    // Stop & upload: "Uploading…" while Rust sends the file, then the link.
    act(() => tauri.emit("recording:started", { recording: true, startedMs: Date.now(), path: "/tmp/d.mp4" }));
    act(() => tauri.emit("recording:uploading"));
    expect(screen.getByText("Uploading…")).toBeTruthy();
    act(() => tauri.emit("recording:stopped", { copied: false, link: "https://socorin.com/s/x" }));
    expect(screen.getByText("Link copied")).toBeTruthy();
    // A failed upload ends like a plain stop (the popover explains).
    act(() => tauri.emit("recording:stopped", { copied: false, link: null }));
    expect(screen.getByText("00:00")).toBeTruthy();
  });

  it("stops, copies or discards through the buttons and survives a failed status query", async () => {
    tauri.handlers.recording_status = () => Promise.reject("gone");
    render(<Recorder />);
    await act(() => vi.advanceTimersByTimeAsync(1));
    fireEvent.click(screen.getByTitle("Stop and copy the video to the clipboard"));
    expect(tauri.calls("stop_recording_copy")).toHaveLength(1);
    // A stop in flight disables the buttons until Rust answers.
    expect((screen.getByTitle("Stop recording") as HTMLButtonElement).disabled).toBe(true);
    act(() => tauri.emit("recording:started", { recording: true, startedMs: Date.now(), path: "/tmp/c.mov" }));
    fireEvent.click(screen.getByTitle("Stop recording"));
    expect(tauri.calls("stop_recording")).toHaveLength(1);
    act(() => tauri.emit("recording:started", { recording: true, startedMs: Date.now(), path: "/tmp/c.mov" }));
    fireEvent.click(screen.getByTitle("Discard the recording"));
    expect(tauri.calls("cancel_recording")).toHaveLength(1);
    act(() => tauri.emit("recording:started", { recording: true, startedMs: Date.now(), path: "/tmp/c.mov" }));
    fireEvent.click(screen.getByTitle("Stop and upload the video, copying its link"));
    // The button's place rides along, for the popover (jsdom: all zeros).
    expect(tauri.calls("stop_recording_upload")).toEqual([{ anchor: { x: 0, y: 0, width: 0, height: 0 } }]);
    expect((screen.getByTitle("Stop recording") as HTMLButtonElement).disabled).toBe(true);
  });

  it("re-enables the buttons when the stop command fails", async () => {
    tauri.handlers.stop_recording = () => Promise.reject("not recording");
    render(<Recorder />);
    await act(() => vi.advanceTimersByTimeAsync(1));
    fireEvent.click(screen.getByTitle("Stop recording"));
    await act(() => vi.advanceTimersByTimeAsync(1));
    expect((screen.getByTitle("Stop recording") as HTMLButtonElement).disabled).toBe(false);
  });
});
