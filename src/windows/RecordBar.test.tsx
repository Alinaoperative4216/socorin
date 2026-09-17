import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { triggerResizeObservers } from "../test/setup";
import { RecordBar } from "./RecordBar";

beforeEach(() => {
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 300 });
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 40 });
});

function mount(props: Partial<Parameters<typeof RecordBar>[0]> = {}) {
  const onStart = vi.fn();
  const onCancel = vi.fn();
  const view = render(
    <RecordBar crop={{ x: 200, y: 200, width: 400, height: 300 }} scale={0.5} hidden={false} onStart={onStart} onCancel={onCancel} {...props} />,
  );
  return { ...view, onStart, onCancel, bar: () => view.container.querySelector(".floating-toolbar") as HTMLElement };
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
    rerender(<RecordBar crop={{ x: 0, y: 0, width: 2048, height: 1536 }} scale={0.5} hidden onStart={onStart} onCancel={onCancel} />);
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
  });
});
