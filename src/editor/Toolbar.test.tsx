import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { Toolbar, TOOLS } from "./Toolbar";
import { COLORS, STROKE_LEVELS } from "./types";

function renderToolbar(overrides: Partial<Parameters<typeof Toolbar>[0]> = {}) {
  const props = {
    tool: "arrow" as const,
    color: COLORS[0],
    strokeLevel: 3,
    canUndo: false,
    canRedo: false,
    hasSelection: false,
    busy: false,
    onTool: vi.fn(),
    onColor: vi.fn(),
    onStroke: vi.fn(),
    onUndo: vi.fn(),
    onRedo: vi.fn(),
    onDelete: vi.fn(),
    onCopy: vi.fn(),
    onSave: vi.fn(),
    onSaveAs: vi.fn(),
    onClose: vi.fn(),
    ...overrides,
  };
  const view = render(<Toolbar {...props} />);
  return { ...view, props };
}

describe("Toolbar (editor window)", () => {
  it("renders every tool, the swatches, the sizes and the zoom controls", () => {
    const onZoom = vi.fn();
    const { props, container } = renderToolbar({ zoom: 0.5, onZoom, onFit: vi.fn() });
    for (const t of TOOLS) expect(screen.getByTitle(`${t.label} (${t.key})`)).toBeTruthy();
    expect(screen.getByTitle("Arrow (A)").className).toContain("active");
    expect(container.querySelectorAll(".swatch:not(.custom)")).toHaveLength(COLORS.length);
    expect(container.querySelectorAll(".size-btn")).toHaveLength(STROKE_LEVELS.length);
    expect(screen.getByText("50%")).toBeTruthy();

    fireEvent.click(screen.getByTitle("Rectangle (R)"));
    expect(props.onTool).toHaveBeenCalledWith("rect");
    fireEvent.click(screen.getByTitle("Zoom in"));
    fireEvent.click(screen.getByTitle("Zoom out"));
    expect(onZoom.mock.calls).toEqual([[1], [-1]]);
    fireEvent.click(screen.getByTitle("Fit to window"));
    expect(props.onFit).toHaveBeenCalled();
  });

  it("does not crash without zoom callbacks", () => {
    renderToolbar();
    fireEvent.click(screen.getByTitle("Zoom in"));
    expect(screen.getByText("100%")).toBeTruthy();
  });

  it("picks colours, custom colours and stroke levels", () => {
    const { props, container } = renderToolbar({ color: "#123456" });
    expect(container.querySelector(".swatch.custom")?.className).toContain("active");
    fireEvent.click(screen.getByTitle(COLORS[3]));
    expect(props.onColor).toHaveBeenCalledWith(COLORS[3]);
    const picker = container.querySelector("input[type=color]") as HTMLInputElement;
    fireEvent.change(picker, { target: { value: "#00ff00" } });
    expect(props.onColor).toHaveBeenCalledWith("#00ff00");
    fireEvent.click(screen.getByTitle("Stroke 5 (8px) — key 5"));
    expect(props.onStroke).toHaveBeenCalledWith(5);
    expect(screen.getByTitle("Stroke 3 (4px) — key 3").className).toContain("active");
  });

  it("enables history and delete buttons from the flags", () => {
    const { props } = renderToolbar({ canUndo: true, canRedo: true, hasSelection: true });
    fireEvent.click(screen.getByTitle("Undo (Ctrl/⌘+Z)"));
    fireEvent.click(screen.getByTitle("Redo (Ctrl/⌘+Shift+Z)"));
    fireEvent.click(screen.getByTitle("Delete selected (Del)"));
    expect(props.onUndo).toHaveBeenCalled();
    expect(props.onRedo).toHaveBeenCalled();
    expect(props.onDelete).toHaveBeenCalled();
  });

  it("disables them otherwise, and the actions while busy", () => {
    const { props } = renderToolbar({ busy: true });
    expect((screen.getByTitle("Undo (Ctrl/⌘+Z)") as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByTitle("Copy to clipboard (Ctrl/⌘+C)") as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByTitle("Close (Esc)"));
    expect(props.onClose).toHaveBeenCalled();
    expect(screen.queryByTitle(/Open in the editor window/)).toBeNull();
  });

  it("runs the actions and keeps focus off the buttons", () => {
    const { props, container } = renderToolbar();
    fireEvent.click(screen.getByTitle("Copy to clipboard (Ctrl/⌘+C)"));
    fireEvent.click(screen.getByTitle("Save to screenshots folder (Ctrl/⌘+S)"));
    fireEvent.click(screen.getByTitle("Save as… (Ctrl/⌘+Shift+S)"));
    expect(props.onCopy).toHaveBeenCalled();
    // Save, Save as, then Copy (the primary one), then Close.
    const labels = screen.getAllByRole("button").map((b) => b.textContent?.trim()).filter((t) => ["Save", "Save as", "Copy", "Close"].includes(t ?? ""));
    expect(labels).toEqual(["Save", "Save as", "Copy", "Close"]);
    expect(props.onSave).toHaveBeenCalled();
    expect(props.onSaveAs).toHaveBeenCalled();
    const down = fireEvent.mouseDown(container.querySelector(".toolbar")!);
    expect(down).toBe(false); // default prevented
  });
});

describe("Toolbar (floating, in-place)", () => {
  it("toggles a tool off when it is clicked again and hides the style row without a tool", () => {
    const { props, container, rerender } = renderToolbar({ floating: true, tool: null, onEdit: vi.fn() });
    expect(container.querySelectorAll(".toolbar-row")).toHaveLength(1);
    fireEvent.click(screen.getByTitle("Pen (P)"));
    expect(props.onTool).toHaveBeenCalledWith("pen");
    fireEvent.click(screen.getByTitle("Open in the editor window (Ctrl/⌘+E)"));
    expect(props.onEdit).toHaveBeenCalled();
    expect(screen.getByTitle("Copy to clipboard and finish (Enter, Ctrl/⌘+C)")).toBeTruthy();

    rerender(<Toolbar {...props} tool="pen" />);
    expect(container.querySelectorAll(".toolbar-row")).toHaveLength(2);
    fireEvent.click(screen.getByTitle("Pen (P)"));
    expect(props.onTool).toHaveBeenLastCalledWith(null);

    rerender(<Toolbar {...props} tool="select" />);
    expect(container.querySelectorAll(".toolbar-row")).toHaveLength(1);
    expect(screen.queryByTitle("Zoom in")).toBeNull();
  });
});
