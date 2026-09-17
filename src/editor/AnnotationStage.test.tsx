import { act, fireEvent, render } from "@testing-library/react";
import type Konva from "konva";
import { useEffect } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { installTauri } from "../test/tauri";
import { AnnotationStage } from "./AnnotationStage";
import { sampleShapes, type BoxShape, type LineShape, type NumberShape, type Shape, type TextShape } from "./types";
import { useAnnotator, type Annotator, type AnnotatorOptions } from "./useAnnotator";

const W = 400;
const H = 300;

function Harness({ expose, image, ...opts }: Partial<AnnotatorOptions> & { expose: (a: Annotator) => void; image: HTMLCanvasElement }) {
  const a = useAnnotator({ width: W, height: H, scale: 1, initialTool: "select", ...opts });
  useEffect(() => {
    expose(a);
  });
  return <AnnotationStage a={a} image={image} width={W} height={H} scale={opts.scale ?? 1} />;
}

function mount(opts: Partial<AnnotatorOptions> = {}) {
  const image = document.createElement("canvas");
  image.width = W;
  image.height = H;
  let a!: Annotator;
  const view = render(<Harness image={image} expose={(x) => (a = x)} {...opts} />);
  const get = () => a;
  const stage = () => get().stageRef.current!;
  const node = <T extends Konva.Node>(id: string) => stage().findOne<T>(`#${id}`)!;
  const setShapes = (shapes: Shape[]) => act(() => get().history.set(shapes));
  return { ...view, get, stage, node, setShapes, image };
}

const base = { color: "#ff3b30", strokeWidth: 4 };
const rect: BoxShape = { ...base, id: "r1", type: "rect", x: 10, y: 20, width: 100, height: 50 };
const ellipse: BoxShape = { ...base, id: "e1", type: "ellipse", x: 10, y: 20, width: 100, height: 50 };
const highlight: BoxShape = { ...base, id: "h1", type: "highlight", x: 0, y: 0, width: 40, height: 10 };
const blur: BoxShape = { ...base, id: "b1", type: "blur", x: 50, y: 50, width: 60, height: 40 };
const arrow: LineShape = { ...base, id: "a1", type: "arrow", points: [0, 0, 50, 50] };
const text: TextShape = { ...base, id: "t1", type: "text", x: 5, y: 5, text: "Hi", fontSize: 20 };
const number: NumberShape = { ...base, id: "n1", type: "number", x: 30, y: 30, n: 2, radius: 12 };

beforeEach(() => {
  installTauri("editor");
});

describe("AnnotationStage", () => {
  it("renders a Konva node for every annotation type", () => {
    const { stage, node, setShapes } = mount();
    setShapes(sampleShapes(W, H));
    for (const s of sampleShapes(W, H)) expect(stage().find(`#${s.id}`).length).toBeGreaterThanOrEqual(0);
    const shapes = stage().find((n: Konva.Node) => !!n.id());
    expect(shapes.length).toBe(9);
    setShapes([blur]);
    expect(node<Konva.Image>("b1").isCached()).toBe(true);
    setShapes([{ ...blur, width: 0.5, height: 0.5 }]);
    expect(stage().find("#b1").length).toBe(1);
  });

  it("switches the cursor class with the tool", () => {
    const { container, get } = mount();
    const holder = () => container.querySelector(".canvas-holder")!.className;
    expect(holder()).toContain("tool-select");
    act(() => get().setTool("text"));
    expect(holder()).toContain("tool-text");
    act(() => get().setTool("pen"));
    expect(holder()).toContain("tool-draw");
    act(() => get().setTool(null));
    expect(holder().trim()).toBe("canvas-holder");
  });

  it("selects shapes on click and attaches the transformer", () => {
    const { node, setShapes, get } = mount();
    setShapes([rect, arrow]);
    act(() => node("r1").fire("click", { evt: {} }));
    expect(get().selectedId).toBe("r1");
    expect(get().trRef.current!.nodes()).toHaveLength(1);
    act(() => node("a1").fire("mousedown", { evt: {} }));
    expect(get().selectedId).toBe("a1");
    expect(get().trRef.current!.nodes()).toHaveLength(0);
    const bound = get().trRef.current!.boundBoxFunc()!;
    const old = { x: 0, y: 0, width: 10, height: 10, rotation: 0 };
    expect(bound(old, { ...old, width: 2 })).toBe(old);
    expect(bound(old, { ...old, width: 20 })).toEqual({ ...old, width: 20 });
  });

  it("reports drags for every shape kind", () => {
    const { node, setShapes, get } = mount();
    setShapes([rect, ellipse, arrow, text, number, blur]);
    const byId = (id: string) => get().shapes.find((s) => s.id === id)!;

    act(() => {
      node("r1").position({ x: 15, y: 25 });
      node("r1").fire("dragend", { evt: {} });
    });
    expect(byId("r1")).toMatchObject({ x: 15, y: 25 });

    act(() => {
      node("e1").position({ x: 100, y: 100 });
      node("e1").fire("dragend", { evt: {} });
    });
    expect(byId("e1")).toMatchObject({ x: 50, y: 75 });

    act(() => {
      node("a1").position({ x: 3, y: 4 });
      node("a1").fire("dragend", { evt: {} });
    });
    expect((byId("a1") as LineShape).points).toEqual([3, 4, 53, 54]);
    expect(node("a1").position()).toEqual({ x: 0, y: 0 });

    act(() => {
      node("t1").position({ x: 8, y: 9 });
      node("t1").fire("dragend", { evt: {} });
    });
    expect(byId("t1")).toMatchObject({ x: 8, y: 9 });

    act(() => {
      node("n1").position({ x: 1, y: 2 });
      node("n1").fire("dragend", { evt: {} });
    });
    expect(byId("n1")).toMatchObject({ x: 1, y: 2 });

    act(() => {
      node("b1").position({ x: 7, y: 7 });
      node("b1").fire("dragend", { evt: {} });
    });
    expect(byId("b1")).toMatchObject({ x: 7, y: 7 });
  });

  it("turns transforms into new sizes and resets the node scale", () => {
    const { node, setShapes, get } = mount();
    setShapes([rect, ellipse, highlight, text, blur]);
    const byId = (id: string) => get().shapes.find((s) => s.id === id)!;

    act(() => {
      node("r1").scale({ x: 2, y: 0.5 });
      node("r1").fire("transformend", { evt: {} });
    });
    expect(byId("r1")).toMatchObject({ x: 10, y: 20, width: 200, height: 25 });
    expect(node("r1").scaleX()).toBe(1);

    act(() => {
      node("e1").scale({ x: 0.5, y: 0.5 });
      node("e1").fire("transformend", { evt: {} });
    });
    expect(byId("e1")).toMatchObject({ x: 35, y: 32.5, width: 50, height: 25 });

    act(() => {
      node("h1").scale({ x: 0.01, y: 0.01 });
      node("h1").fire("transformend", { evt: {} });
    });
    expect(byId("h1")).toMatchObject({ width: 2, height: 2 });

    act(() => {
      node("t1").scale({ x: 1.5, y: 2 });
      node("t1").fire("transformend", { evt: {} });
    });
    expect((byId("t1") as TextShape).fontSize).toBe(40);

    act(() => {
      node("b1").scale({ x: 2, y: 2 });
      node("b1").fire("transformend", { evt: {} });
    });
    expect(byId("b1")).toMatchObject({ width: 120, height: 80 });
  });

  it("edits text in a textarea and commits with Enter, Escape or blur", async () => {
    vi.useFakeTimers();
    const { node, setShapes, get, container } = mount();
    setShapes([text]);
    act(() => node("t1").fire("dblclick", { evt: {} }));
    expect(get().editingId).toBe("t1");
    expect(node("t1").visible()).toBe(false);
    const ta = container.querySelector("textarea.text-editor") as HTMLTextAreaElement;
    expect(ta.defaultValue).toBe("Hi");
    await act(() => vi.advanceTimersByTimeAsync(1));
    expect(document.activeElement).toBe(ta);

    fireEvent.input(ta, { target: { value: "Hello there" } });
    expect(ta.style.width).toMatch(/px$/);
    fireEvent.keyDown(ta, { key: "Enter", shiftKey: true });
    expect(get().editingId).toBe("t1");
    fireEvent.keyDown(ta, { key: "Enter" });
    expect(get().editingId).toBeNull();
    expect((get().shapes[0] as TextShape).text).toBe("Hello there");
    expect(node("t1").visible()).toBe(true);

    act(() => node("t1").fire("dbltap", { evt: {} }));
    const ta2 = container.querySelector("textarea.text-editor") as HTMLTextAreaElement;
    fireEvent.keyDown(ta2, { key: "Escape" });
    expect(get().editingId).toBeNull();

    act(() => get().startEditText("t1"));
    const ta3 = container.querySelector("textarea.text-editor") as HTMLTextAreaElement;
    fireEvent.change(ta3, { target: { value: "" } });
    fireEvent.blur(ta3);
    expect(get().shapes).toHaveLength(0);
  });

  it("draws through the stage's own mouse events and shows the draft", () => {
    const { stage, get, container } = mount({ initialTool: "rect" });
    const content = stage().content;
    fireEvent.mouseDown(content, { clientX: 10, clientY: 10, button: 0 });
    expect(get().draft).toMatchObject({ type: "rect", x: 10, y: 10 });
    fireEvent.mouseMove(content, { clientX: 60, clientY: 40 });
    expect(get().draft).toMatchObject({ width: 50, height: 30 });
    expect(stage().find((n: Konva.Node) => n.id() === get().draft!.id)).toHaveLength(1);
    fireEvent.mouseUp(content, { clientX: 60, clientY: 40, button: 0 });
    expect(get().shapes).toHaveLength(1);
    expect(get().draft).toBeNull();

    fireEvent.mouseDown(content, { clientX: 100, clientY: 100, button: 0 });
    fireEvent.mouseMove(content, { clientX: 130, clientY: 130 });
    fireEvent.mouseLeave(content);
    expect(get().shapes).toHaveLength(2);
    expect(container.querySelectorAll("canvas").length).toBeGreaterThan(0);
  });
});
