import { act, renderHook } from "@testing-library/react";
import type Konva from "konva";
import type { KonvaEventObject } from "konva/lib/Node";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { isMac } from "../lib/hotkey";
import { PNG_BYTES } from "../test/setup";
import { installTauri, type TauriMock } from "../test/tauri";
import { COLORS, type BoxShape, type LineShape, type PenShape, type TextShape } from "./types";
import { useAnnotator, type AnnotatorOptions } from "./useAnnotator";

/** A stage that only knows where the pointer is and how zoomed it is. */
function fakeStage(pointer: { x: number; y: number } | null, scale = 1) {
  const canvas = document.createElement("canvas");
  const stage = {
    getPointerPosition: () => pointer,
    scaleX: () => scale,
    toCanvas: vi.fn(() => canvas),
    getStage: () => stage,
  };
  return stage as unknown as Konva.Stage & { toCanvas: ReturnType<typeof vi.fn> };
}

function mouse(target: unknown, button = 0) {
  return {
    target,
    evt: { button, preventDefault: vi.fn() },
  } as unknown as KonvaEventObject<MouseEvent>;
}

const mod = isMac ? { metaKey: true } : { ctrlKey: true };
const keyEvent = (init: KeyboardEventInit) => {
  const e = new KeyboardEvent("keydown", init);
  vi.spyOn(e, "preventDefault");
  return e;
};

function setup(opts: Partial<AnnotatorOptions> = {}, pointer: { x: number; y: number } | null = { x: 10, y: 10 }) {
  const hook = renderHook((p: Partial<AnnotatorOptions>) => useAnnotator({ width: 400, height: 300, scale: 1, ...p }), {
    initialProps: opts,
  });
  const stage = fakeStage(pointer, opts.scale ?? 1);
  hook.result.current.stageRef.current = stage;
  const setPointer = (p: { x: number; y: number } | null) => {
    stage.getPointerPosition = () => p;
  };
  return { ...hook, stage, setPointer };
}

let tauri: TauriMock;
beforeEach(() => {
  tauri = installTauri("editor");
});

describe("useAnnotator defaults and style", () => {
  it("starts with the arrow tool, the first colour and stroke level 3", () => {
    const { result } = setup();
    expect(result.current.tool).toBe("arrow");
    expect(result.current.color).toBe(COLORS[0]);
    expect(result.current.strokeLevel).toBe(3);
    expect(result.current.strokeWidth).toBe(4);
    expect(result.current.shapes).toEqual([]);
  });

  it("applies the remembered style, scaled by the pixel ratio, and follows later changes", () => {
    const { result, rerender } = setup({ initialTool: null, initialColor: "#123456", initialStrokeLevel: 5, pixelRatio: 2 });
    expect(result.current.tool).toBeNull();
    expect(result.current.color).toBe("#123456");
    expect(result.current.strokeLevel).toBe(5);
    expect(result.current.strokeWidth).toBe(16);
    rerender({ initialTool: null, initialColor: "#abcdef", initialStrokeLevel: 9, pixelRatio: 2 });
    expect(result.current.color).toBe("#abcdef");
    expect(result.current.strokeLevel).toBe(5);
  });

  it("persists colour and stroke changes once they settle", async () => {
    vi.useFakeTimers();
    const { result } = setup();
    act(() => {
      result.current.setColor("#00ff00");
      result.current.setStrokeLevel(9);
    });
    expect(result.current.color).toBe("#00ff00");
    expect(result.current.strokeLevel).toBe(5);
    expect(tauri.calls("update_settings")).toHaveLength(0);
    await act(() => vi.advanceTimersByTimeAsync(400));
    expect(tauri.calls("update_settings")).toEqual([{ patch: { annotationColor: "#00ff00", annotationStroke: 5 } }]);
  });

  it("logs a failed style save", async () => {
    vi.useFakeTimers();
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    tauri.handlers.update_settings = () => Promise.reject(new Error("disk full"));
    const { result } = setup();
    act(() => result.current.setColor("#000000"));
    await act(() => vi.advanceTimersByTimeAsync(400));
    expect(error).toHaveBeenCalled();
  });
});

describe("drawing gestures", () => {
  it("draws a rectangle, normalising a backwards drag", () => {
    const { result, setPointer } = setup({ initialTool: "rect" }, { x: 100, y: 80 });
    act(() => result.current.onMouseDown(mouse({})));
    expect(result.current.draft).toMatchObject({ type: "rect", x: 100, y: 80, width: 0, height: 0 });
    setPointer({ x: 40, y: 50 });
    act(() => result.current.onMouseMove());
    expect(result.current.draft).toMatchObject({ width: -60, height: -30 });
    act(() => result.current.onMouseUp());
    expect(result.current.draft).toBeNull();
    expect(result.current.shapes).toHaveLength(1);
    expect(result.current.shapes[0]).toMatchObject({ type: "rect", x: 40, y: 50, width: 60, height: 30, strokeWidth: 4 });
  });

  it("drops boxes and lines that are too small", () => {
    const { result, setPointer } = setup({ initialTool: "blur" }, { x: 10, y: 10 });
    act(() => result.current.onMouseDown(mouse({})));
    setPointer({ x: 12, y: 30 });
    act(() => result.current.onMouseMove());
    act(() => result.current.onMouseUp());
    expect(result.current.shapes).toHaveLength(0);

    act(() => result.current.setTool("line"));
    act(() => result.current.onMouseDown(mouse({})));
    setPointer({ x: 13, y: 31 });
    act(() => result.current.onMouseMove());
    act(() => result.current.onMouseUp());
    expect(result.current.shapes).toHaveLength(0);
  });

  it("draws arrows and lines from the press to the release point", () => {
    const { result, setPointer } = setup({ initialTool: "arrow" }, { x: 0, y: 0 });
    act(() => result.current.onMouseDown(mouse({})));
    setPointer({ x: 30, y: 40 });
    act(() => result.current.onMouseMove());
    setPointer({ x: 60, y: 80 });
    act(() => result.current.onMouseMove());
    act(() => result.current.onMouseUp());
    expect((result.current.shapes[0] as LineShape).points).toEqual([0, 0, 60, 80]);
  });

  it("collects pen points and needs at least two of them", () => {
    const { result, setPointer } = setup({ initialTool: "pen" }, { x: 1, y: 1 });
    act(() => result.current.onMouseDown(mouse({})));
    act(() => result.current.onMouseUp());
    expect(result.current.shapes).toHaveLength(0);

    act(() => result.current.onMouseDown(mouse({})));
    setPointer({ x: 2, y: 2 });
    act(() => result.current.onMouseMove());
    setPointer({ x: 3, y: 5 });
    act(() => result.current.onMouseMove());
    act(() => result.current.onMouseUp());
    expect((result.current.shapes[0] as PenShape).points).toEqual([1, 1, 2, 2, 3, 5]);
  });

  it("clamps the pointer to the surface and divides by the zoom", () => {
    const { result, setPointer } = setup({ initialTool: "rect", scale: 2 }, { x: -20, y: -20 });
    act(() => result.current.onMouseDown(mouse({})));
    setPointer({ x: 5000, y: 100 });
    act(() => result.current.onMouseMove());
    expect(result.current.draft).toMatchObject({ x: 0, y: 0, width: 400, height: 50 });
    act(() => result.current.onMouseUp());
    expect(result.current.shapes[0]).toMatchObject({ width: 400, height: 50 });
  });

  it("places text and number markers on click", () => {
    const { result } = setup({ initialTool: "text", pixelRatio: 2 }, { x: 5, y: 6 });
    const e = mouse({});
    act(() => result.current.onMouseDown(e));
    expect(e.evt.preventDefault).toHaveBeenCalled();
    const text = result.current.shapes[0] as TextShape;
    expect(text).toMatchObject({ type: "text", x: 5, y: 6, text: "" });
    expect(text.fontSize).toBe(Math.round(20 + 8 * 3.5));
    expect(result.current.editingId).toBe(text.id);
    expect(result.current.editingShape).toEqual(text);
    // While editing, the stage ignores further presses.
    act(() => result.current.onMouseDown(mouse({})));
    expect(result.current.shapes).toHaveLength(1);

    act(() => result.current.commitText("Hello   "));
    expect(result.current.editingId).toBeNull();
    expect((result.current.shapes[0] as TextShape).text).toBe("Hello");

    act(() => result.current.setTool("number"));
    act(() => result.current.onMouseDown(mouse({})));
    act(() => result.current.onMouseDown(mouse({})));
    const numbers = result.current.shapes.filter((s) => s.type === "number");
    expect(numbers.map((s) => (s as { n: number }).n)).toEqual([1, 2]);
  });

  it("removes a text that was left empty and can re-open one for editing", () => {
    const { result } = setup({ initialTool: "text" });
    act(() => result.current.onMouseDown(mouse({})));
    const id = result.current.shapes[0].id;
    act(() => result.current.commitText("   "));
    expect(result.current.shapes).toHaveLength(0);
    act(() => result.current.commitText("ignored"));
    act(() => result.current.startEditText(id));
    expect(result.current.editingId).toBe(id);
    expect(result.current.editingShape).toBeUndefined();
  });

  it("ignores presses without a tool, with other buttons or without a pointer", () => {
    const { result, setPointer } = setup({ initialTool: null });
    act(() => result.current.onMouseDown(mouse({})));
    expect(result.current.draft).toBeNull();

    act(() => result.current.setTool("rect"));
    act(() => result.current.onMouseDown(mouse({}, 2)));
    expect(result.current.draft).toBeNull();

    setPointer(null);
    act(() => result.current.onMouseDown(mouse({})));
    expect(result.current.draft).toBeNull();
    act(() => result.current.onMouseMove());
    act(() => result.current.onMouseUp());
    expect(result.current.shapes).toHaveLength(0);
  });

  it("keeps the draft when the pointer leaves the stage mid-gesture", () => {
    const { result, setPointer } = setup({ initialTool: "ellipse" }, { x: 10, y: 10 });
    act(() => result.current.onMouseDown(mouse({})));
    setPointer(null);
    act(() => result.current.onMouseMove());
    expect(result.current.draft).toMatchObject({ width: 0, height: 0 });
  });
});

describe("selection", () => {
  it("clears the selection when clicking the empty stage or switching tools", () => {
    const { result, stage } = setup({ initialTool: "select" });
    act(() => result.current.setSelectedId("abc"));
    act(() => result.current.onMouseDown(mouse({ getStage: () => stage })));
    expect(result.current.selectedId).toBe("abc");
    act(() => result.current.onMouseDown(mouse(stage)));
    expect(result.current.selectedId).toBeNull();

    act(() => result.current.setSelectedId("abc"));
    act(() => result.current.setTool("rect"));
    expect(result.current.selectedId).toBeNull();
  });

  it("updates and deletes shapes", () => {
    const { result } = setup({ initialTool: "rect" }, { x: 0, y: 0 });
    act(() => result.current.onMouseDown(mouse({})));
    result.current.stageRef.current!.getPointerPosition = () => ({ x: 50, y: 50 });
    act(() => result.current.onMouseMove());
    act(() => result.current.onMouseUp());
    const shape = result.current.shapes[0] as BoxShape;
    act(() => result.current.updateShape({ ...shape, x: 99 }));
    expect((result.current.shapes[0] as BoxShape).x).toBe(99);

    act(() => result.current.deleteSelected());
    expect(result.current.shapes).toHaveLength(1);
    act(() => result.current.setTool("select"));
    act(() => result.current.setSelectedId(shape.id));
    act(() => result.current.deleteSelected());
    expect(result.current.shapes).toHaveLength(0);
    expect(result.current.selectedId).toBeNull();

    act(() => result.current.history.undo());
    expect(result.current.shapes).toHaveLength(1);
    act(() => result.current.reset());
    expect(result.current.shapes).toHaveLength(0);
    expect(result.current.history.canUndo).toBe(false);
  });

  it("attaches the transformer to transformable selected shapes only", () => {
    const { result } = setup({ initialTool: "select" });
    const tr = {
      nodes: vi.fn(),
      keepRatio: vi.fn(),
      enabledAnchors: vi.fn(),
      getLayer: () => ({ batchDraw: vi.fn() }),
    };
    const node = {};
    const layer = { findOne: vi.fn(() => node) };
    result.current.trRef.current = tr as unknown as Konva.Transformer;
    result.current.layerRef.current = layer as unknown as Konva.Layer;
    const base = { id: "", color: "#fff", strokeWidth: 2 };
    act(() =>
      result.current.history.set([
        { ...base, id: "r", type: "rect", x: 0, y: 0, width: 10, height: 10 },
        { ...base, id: "t", type: "text", x: 0, y: 0, text: "x", fontSize: 12 },
        { ...base, id: "a", type: "arrow", points: [0, 0, 5, 5] },
      ]),
    );
    act(() => result.current.setSelectedId("r"));
    expect(tr.nodes).toHaveBeenLastCalledWith([node]);
    expect(tr.keepRatio).toHaveBeenLastCalledWith(false);
    expect(tr.enabledAnchors.mock.lastCall?.[0]).toHaveLength(8);

    act(() => result.current.setSelectedId("t"));
    expect(tr.keepRatio).toHaveBeenLastCalledWith(true);
    expect(tr.enabledAnchors.mock.lastCall?.[0]).toHaveLength(4);

    act(() => result.current.setSelectedId("a"));
    expect(tr.nodes).toHaveBeenLastCalledWith([]);

    act(() => result.current.setSelectedId("nope"));
    expect(layer.findOne).not.toHaveBeenCalledWith("#nope");
  });
});

describe("keyboard", () => {
  function withShape() {
    const s = setup({ initialTool: "rect" }, { x: 0, y: 0 });
    act(() => s.result.current.onMouseDown(mouse({})));
    s.setPointer({ x: 30, y: 30 });
    act(() => s.result.current.onMouseMove());
    act(() => s.result.current.onMouseUp());
    return s;
  }

  it("handles undo / redo", () => {
    const { result } = withShape();
    let e = keyEvent({ key: "z", ...mod });
    let handled = false;
    act(() => {
      handled = result.current.handleKey(e);
    });
    expect(handled).toBe(true);
    expect(e.preventDefault).toHaveBeenCalled();
    expect(result.current.shapes).toHaveLength(0);

    e = keyEvent({ key: "Z", shiftKey: true, ...mod });
    act(() => void result.current.handleKey(e));
    expect(result.current.shapes).toHaveLength(1);

    act(() => void result.current.handleKey(keyEvent({ key: "z", ...mod })));
    act(() => void result.current.handleKey(keyEvent({ key: "y", ...mod })));
    expect(result.current.shapes).toHaveLength(1);
  });

  it("deselects with Escape and deletes with Delete / Backspace", () => {
    const { result } = withShape();
    const id = result.current.shapes[0].id;
    act(() => result.current.setTool("select"));
    act(() => result.current.setSelectedId(id));
    let handled = false;
    act(() => {
      handled = result.current.handleKey(keyEvent({ key: "Escape" }));
    });
    expect(handled).toBe(true);
    expect(result.current.selectedId).toBeNull();
    expect(result.current.handleKey(keyEvent({ key: "Escape" }))).toBe(false);

    act(() => result.current.setSelectedId(id));
    act(() => void result.current.handleKey(keyEvent({ key: "Backspace" })));
    expect(result.current.shapes).toHaveLength(0);
    expect(result.current.handleKey(keyEvent({ key: "Delete" }))).toBe(false);
  });

  it("switches tools and stroke levels with plain keys", () => {
    const { result } = setup();
    act(() => void result.current.handleKey(keyEvent({ key: "e" })));
    expect(result.current.tool).toBe("ellipse");
    act(() => void result.current.handleKey(keyEvent({ key: "V" })));
    expect(result.current.tool).toBe("select");
    act(() => void result.current.handleKey(keyEvent({ key: "5" })));
    expect(result.current.strokeLevel).toBe(5);
    expect(result.current.handleKey(keyEvent({ key: "9" }))).toBe(false);
    expect(result.current.handleKey(keyEvent({ key: "e", altKey: true }))).toBe(false);
    expect(result.current.handleKey(keyEvent({ key: "F1" }))).toBe(false);
  });

  it("gives the keyboard to the textarea while editing", () => {
    const { result } = setup({ initialTool: "text" });
    act(() => result.current.onMouseDown(mouse({})));
    expect(result.current.handleKey(keyEvent({ key: "r" }))).toBe(true);
    expect(result.current.tool).toBe("text");
  });
});

describe("renderPng", () => {
  it("exports at image resolution using the stage's live zoom", async () => {
    const { result, stage } = setup({ scale: 0.5 });
    const tr = { visible: vi.fn() };
    result.current.trRef.current = tr as unknown as Konva.Transformer;
    const png = await result.current.renderPng();
    expect(Array.from(png)).toEqual(Array.from(PNG_BYTES));
    expect(stage.toCanvas).toHaveBeenCalledWith({ x: 0, y: 0, width: 200, height: 150, pixelRatio: 2 });
    expect(tr.visible.mock.calls).toEqual([[false], [true]]);
  });

  it("treats a zero zoom as 1 and fails without a stage", async () => {
    const { result, stage } = setup({ scale: 0 });
    await result.current.renderPng();
    expect(stage.toCanvas).toHaveBeenCalledWith({ x: 0, y: 0, width: 400, height: 300, pixelRatio: 1 });
    result.current.stageRef.current = null;
    await expect(result.current.renderPng()).rejects.toThrow("nothing to export");
  });

  it("rejects when the canvas cannot be encoded", async () => {
    const { result, stage } = setup();
    const canvas = document.createElement("canvas");
    canvas.toBlob = (cb: BlobCallback) => cb(null);
    stage.toCanvas.mockReturnValue(canvas);
    await expect(result.current.renderPng()).rejects.toThrow("toBlob failed");
  });
});
