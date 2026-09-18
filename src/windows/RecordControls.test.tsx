import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { formatElapsed, RecordControls } from "./RecordControls";

describe("RecordControls", () => {
  it("formats the clock like the tray", () => {
    expect(formatElapsed(0)).toBe("00:00");
    expect(formatElapsed(-5)).toBe("00:00");
    expect(formatElapsed(65_999)).toBe("01:05");
    expect(formatElapsed(3_725_000)).toBe("1:02:05");
  });

  it("offers Record / Cancel before the recording", () => {
    const onRecord = vi.fn();
    const onCancel = vi.fn();
    const { container } = render(<RecordControls phase="ready" elapsedMs={0} onRecord={onRecord} onCancel={onCancel} />);
    expect(screen.getByText("Adjust the area, then")).toBeTruthy();
    // The logo leads the bar in every phase, before the hint / clock.
    const logo = container.querySelector(".toolbar-row")!.firstElementChild as HTMLImageElement;
    expect(logo.className).toBe("toolbar-logo");
    expect(logo.getAttribute("alt")).toBe("");
    expect(logo.draggable).toBe(false);
    fireEvent.click(screen.getByTitle("Start recording (Enter)"));
    fireEvent.click(screen.getByTitle("Cancel (Esc)"));
    expect(onRecord).toHaveBeenCalledTimes(1);
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(screen.queryByTitle("Stop recording")).toBeNull();
    // Buttons never take the keyboard focus away from the overlay.
    expect(fireEvent.mouseDown(container.querySelector(".record-bar")!)).toBe(false);
  });

  it("shows the clock with Stop & copy / Stop & upload / Stop / Cancel while recording", () => {
    const onStop = vi.fn();
    const onStopCopy = vi.fn();
    const onStopUpload = vi.fn();
    const onCancel = vi.fn();
    const { container, rerender } = render(
      <RecordControls phase="recording" elapsedMs={65_000} onStop={onStop} onStopCopy={onStopCopy} onStopUpload={onStopUpload} onCancel={onCancel} />,
    );
    expect(screen.getByText("01:05")).toBeTruthy();
    expect(container.querySelector(".toolbar-row")!.firstElementChild!.className).toBe("toolbar-logo");
    expect(container.querySelector(".rec-dot")?.className).toContain("live");
    fireEvent.click(screen.getByTitle("Stop and copy the video to the clipboard"));
    fireEvent.click(screen.getByTitle("Stop and upload the video, copying its link"));
    fireEvent.click(screen.getByTitle("Stop recording"));
    fireEvent.click(screen.getByTitle("Discard the recording"));
    expect(onStopCopy).toHaveBeenCalledTimes(1);
    expect(onStopUpload).toHaveBeenCalledTimes(1);
    expect(onStopUpload).toHaveBeenCalledWith({ x: 0, y: 0, width: 0, height: 0 }); // the button, for the popover
    expect(onStop).toHaveBeenCalledTimes(1);
    expect(onCancel).toHaveBeenCalledTimes(1);
    const labels = screen.getAllByRole("button").map((b) => b.textContent?.trim());
    expect(labels).toEqual(["Stop & copy", "Stop & upload", "Stop", "Cancel"]);

    // Before the encoder is up the dot does not pulse yet; while stopping
    // the buttons are disabled; after Stop & copy the bar says so.
    rerender(<RecordControls phase="starting" elapsedMs={0} />);
    expect(container.querySelector(".rec-dot")?.className).not.toContain("live");
    rerender(<RecordControls phase="stopping" elapsedMs={1000} onStop={onStop} />);
    expect((screen.getByTitle("Stop recording") as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByTitle("Stop recording"));
    expect(onStop).toHaveBeenCalledTimes(1);
    rerender(<RecordControls phase="copied" elapsedMs={0} />);
    expect(screen.getByText("Copied to clipboard")).toBeTruthy();
    // Stop & upload: "Uploading…" with the buttons off, then "Link copied".
    rerender(<RecordControls phase="uploading" elapsedMs={0} onStopUpload={onStopUpload} />);
    expect(screen.getByText("Uploading…")).toBeTruthy();
    expect(container.querySelector(".rec-busy")).toBeTruthy();
    expect((screen.getByTitle("Stop and upload the video, copying its link") as HTMLButtonElement).disabled).toBe(true);
    rerender(<RecordControls phase="shared" elapsedMs={0} />);
    expect(screen.getByText("Link copied")).toBeTruthy();
    expect((screen.getByTitle("Stop recording") as HTMLButtonElement).disabled).toBe(true);
  });

  it("switches the microphone before the recording and shows it during", () => {
    const onToggleMic = vi.fn();
    const { rerender } = render(
      <RecordControls phase="ready" elapsedMs={0} mic={{ on: true, name: "Jabra", issue: null }} onToggleMic={onToggleMic} />,
    );
    const button = screen.getByTitle("Microphone: Jabra — click to record without sound") as HTMLButtonElement;
    expect(button.className).toContain("rec-mic on");
    expect(button.getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(button);
    expect(onToggleMic).toHaveBeenCalledTimes(1);
    // The mic slot sits left of Record, so Stop lands where Record was.
    expect(screen.getAllByRole("button").map((b) => b.textContent?.trim())).toEqual(["", "Record", "Cancel"]);

    rerender(<RecordControls phase="ready" elapsedMs={0} mic={{ on: false, name: null, issue: null }} onToggleMic={onToggleMic} />);
    const off = screen.getByTitle("Microphone off — click to record sound");
    expect(off.className).toContain("rec-mic off");
    expect(off.getAttribute("aria-pressed")).toBe("false");
    rerender(<RecordControls phase="ready" elapsedMs={0} mic={{ on: true, name: "Built-in", issue: "Jabra is not connected; recording with Built-in." }} />);
    expect(screen.getByTitle(/Jabra is not connected/).className).toContain("on issue");
    // Not known yet (the microphones are still being listed): a plain icon.
    rerender(<RecordControls phase="ready" elapsedMs={0} />);
    expect(screen.getByTitle("Microphone").className).toContain("unknown");

    // During the recording an indicator, not a button, with what Rust reported.
    rerender(<RecordControls phase="recording" elapsedMs={0} mic={{ on: true, name: "Jabra", issue: null }} />);
    const indicator = screen.getByLabelText("Microphone");
    expect(indicator.tagName).toBe("SPAN");
    expect(indicator.getAttribute("title")).toBe("Microphone: Jabra");
    expect(screen.getAllByRole("button").map((b) => b.textContent?.trim())).toEqual(["Stop & copy", "Stop & upload", "Stop", "Cancel"]);
    rerender(<RecordControls phase="recording" elapsedMs={0} mic={{ on: false, name: null, issue: "Microphone access is off." }} />);
    expect(screen.getByLabelText("Microphone").className).toContain("off issue");
    expect(screen.getByLabelText("Microphone").getAttribute("title")).toBe("Microphone off: Microphone access is off.");
    rerender(<RecordControls phase="starting" elapsedMs={0} />);
    expect(screen.getByLabelText("Microphone").className).toContain("unknown");
    expect(screen.getByLabelText("Microphone").getAttribute("title")).toBeNull();
  });
});
