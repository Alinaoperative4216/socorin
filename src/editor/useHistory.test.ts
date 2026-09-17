import { act, renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { useHistory } from "./useHistory";

describe("useHistory", () => {
  it("tracks past and future around the present value", () => {
    const { result } = renderHook(() => useHistory<number>(0));
    expect(result.current.value).toBe(0);
    expect(result.current.canUndo).toBe(false);
    expect(result.current.canRedo).toBe(false);

    act(() => result.current.set(1));
    act(() => result.current.set((v) => v + 10));
    expect(result.current.value).toBe(11);
    expect(result.current.canUndo).toBe(true);

    act(() => result.current.undo());
    expect(result.current.value).toBe(1);
    expect(result.current.canRedo).toBe(true);

    act(() => result.current.redo());
    expect(result.current.value).toBe(11);
    expect(result.current.canRedo).toBe(false);
  });

  it("ignores no-op sets, undo at the start and redo at the end", () => {
    const { result } = renderHook(() => useHistory<string>("a"));
    act(() => result.current.set("a"));
    expect(result.current.canUndo).toBe(false);
    act(() => result.current.undo());
    expect(result.current.value).toBe("a");
    act(() => result.current.redo());
    expect(result.current.value).toBe("a");
  });

  it("drops the redo stack on a new change and caps the past", () => {
    const { result } = renderHook(() => useHistory<number>(0));
    act(() => result.current.set(1));
    act(() => result.current.undo());
    expect(result.current.canRedo).toBe(true);
    act(() => result.current.set(2));
    expect(result.current.canRedo).toBe(false);

    for (let i = 3; i < 150; i++) act(() => result.current.set(i));
    let undos = 0;
    while (result.current.canUndo) {
      act(() => result.current.undo());
      undos += 1;
    }
    expect(undos).toBe(100);
  });

  it("reset replaces everything", () => {
    const { result } = renderHook(() => useHistory<number[]>([]));
    act(() => result.current.set([1]));
    act(() => result.current.reset([9, 9]));
    expect(result.current.value).toEqual([9, 9]);
    expect(result.current.canUndo).toBe(false);
    expect(result.current.canRedo).toBe(false);
  });
});
