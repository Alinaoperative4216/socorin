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
    fireEvent.click(screen.getByTitle("Start recording (Enter)"));
    fireEvent.click(screen.getByTitle("Cancel (Esc)"));
    expect(onRecord).toHaveBeenCalledTimes(1);
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(screen.queryByTitle("Stop recording")).toBeNull();
    // Buttons never take the keyboard focus away from the overlay.
    expect(fireEvent.mouseDown(container.querySelector(".record-bar")!)).toBe(false);
  });

  it("shows the clock with Stop & copy / Stop / Cancel while recording", () => {
    const onStop = vi.fn();
    const onStopCopy = vi.fn();
    const onCancel = vi.fn();
    const { container, rerender } = render(
      <RecordControls phase="recording" elapsedMs={65_000} onStop={onStop} onStopCopy={onStopCopy} onCancel={onCancel} />,
    );
    expect(screen.getByText("01:05")).toBeTruthy();
    expect(container.querySelector(".rec-dot")?.className).toContain("live");
    fireEvent.click(screen.getByTitle("Stop and copy the video to the clipboard"));
    fireEvent.click(screen.getByTitle("Stop recording"));
    fireEvent.click(screen.getByTitle("Discard the recording"));
    expect(onStopCopy).toHaveBeenCalledTimes(1);
    expect(onStop).toHaveBeenCalledTimes(1);
    expect(onCancel).toHaveBeenCalledTimes(1);

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
  });
});
