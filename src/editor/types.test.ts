import { describe, expect, it } from "vitest";
import {
  badgeRadiusFor,
  clampStrokeLevel,
  COLORS,
  fontSizeFor,
  newId,
  sampleShapes,
  STROKE_LEVELS,
  strokeWidthFor,
} from "./types";

describe("stroke levels", () => {
  it("clamps levels into 1..5 and falls back to the default", () => {
    expect(clampStrokeLevel(0)).toBe(3); // 0 means "unset"
    expect(clampStrokeLevel(-4)).toBe(1);
    expect(clampStrokeLevel(3.4)).toBe(3);
    expect(clampStrokeLevel(99)).toBe(STROKE_LEVELS.length);
    expect(clampStrokeLevel(NaN)).toBe(3);
  });

  it("scales the preset by the pixel ratio", () => {
    expect(strokeWidthFor(1)).toBe(2);
    expect(strokeWidthFor(5)).toBe(8);
    expect(strokeWidthFor(3, 2)).toBe(8);
    expect(strokeWidthFor(42, 0.5)).toBe(4);
  });

  it("derives text and badge sizes from the stroke", () => {
    expect(fontSizeFor(4)).toBe(24);
    expect(fontSizeFor(4, 2)).toBe(34);
    expect(badgeRadiusFor(4)).toBe(14);
    expect(badgeRadiusFor(2, 2)).toBe(19);
  });
});

describe("newId", () => {
  it("never repeats", () => {
    const ids = new Set(Array.from({ length: 50 }, () => newId()));
    expect(ids.size).toBe(50);
  });
});

describe("sampleShapes", () => {
  it("returns one of every annotation type inside the given box", () => {
    const shapes = sampleShapes(1000, 500);
    expect(shapes.map((s) => s.type)).toEqual(["rect", "ellipse", "arrow", "line", "pen", "text", "highlight", "number", "blur"]);
    expect(new Set(shapes.map((s) => s.id)).size).toBe(shapes.length);
    for (const s of shapes) {
      expect(COLORS).toContain(s.color);
      if ("x" in s) {
        expect(s.x).toBeGreaterThanOrEqual(0);
        expect(s.x).toBeLessThanOrEqual(1000);
      }
      if ("points" in s) for (const v of s.points) expect(v).toBeLessThanOrEqual(1000);
    }
  });
});
