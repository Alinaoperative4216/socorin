export type ToolId =
  | "select"
  | "rect"
  | "ellipse"
  | "arrow"
  | "line"
  | "pen"
  | "text"
  | "highlight"
  | "number"
  | "blur";

export interface ShapeBase {
  id: string;
  color: string;
  strokeWidth: number;
}

export interface BoxShape extends ShapeBase {
  type: "rect" | "ellipse" | "highlight" | "blur";
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface LineShape extends ShapeBase {
  type: "arrow" | "line";
  /** [x1, y1, x2, y2] in image pixels. */
  points: number[];
}

export interface PenShape extends ShapeBase {
  type: "pen";
  points: number[];
}

export interface TextShape extends ShapeBase {
  type: "text";
  x: number;
  y: number;
  text: string;
  fontSize: number;
}

export interface NumberShape extends ShapeBase {
  type: "number";
  x: number;
  y: number;
  n: number;
  radius: number;
}

export type Shape = BoxShape | LineShape | PenShape | TextShape | NumberShape;

export const BOX_TOOLS: ReadonlySet<string> = new Set(["rect", "ellipse", "highlight", "blur"]);
export const LINE_TOOLS: ReadonlySet<string> = new Set(["arrow", "line"]);

export const COLORS = ["#ff3b30", "#ff9500", "#ffd60a", "#34c759", "#0a84ff", "#bf5af2", "#ffffff", "#111111"];

/**
 * Stroke presets in *logical* (screen) pixels, level 1..5. Shapes store the
 * width in image pixels: level × the image's pixel ratio, so a stroke looks
 * the same on a Retina screen as on a 1× one and the exported PNG matches
 * what was on screen.
 */
export const STROKE_LEVELS = [2, 3, 4, 6, 8];
export const DEFAULT_STROKE_LEVEL = 3;

export function clampStrokeLevel(level: number): number {
  return Math.min(STROKE_LEVELS.length, Math.max(1, Math.round(level) || DEFAULT_STROKE_LEVEL));
}

export function strokeWidthFor(level: number, pixelRatio = 1): number {
  return STROKE_LEVELS[clampStrokeLevel(level) - 1] * pixelRatio;
}

let counter = 0;
export function newId(): string {
  counter += 1;
  return `${Date.now().toString(36)}-${counter}`;
}

/** Initial text size for a stroke width (both in image pixels). */
export function fontSizeFor(strokeWidth: number, pixelRatio = 1): number {
  return Math.round(10 * pixelRatio + strokeWidth * 3.5);
}

/** Numbered-marker badge radius for a stroke width (both in image pixels). */
export function badgeRadiusFor(strokeWidth: number, pixelRatio = 1): number {
  return Math.round(8 * pixelRatio + strokeWidth * 1.5);
}

/** One of every annotation type, for automated end-to-end runs. */
export function sampleShapes(w: number, h: number): Shape[] {
  const base = { color: COLORS[0], strokeWidth: 4 };
  return [
    { ...base, id: newId(), type: "rect", x: w * 0.1, y: h * 0.1, width: w * 0.3, height: h * 0.25 },
    { ...base, id: newId(), type: "ellipse", color: COLORS[4], x: w * 0.55, y: h * 0.1, width: w * 0.3, height: h * 0.25 },
    { ...base, id: newId(), type: "arrow", color: COLORS[3], points: [w * 0.1, h * 0.9, w * 0.4, h * 0.5] },
    { ...base, id: newId(), type: "line", color: COLORS[1], points: [w * 0.5, h * 0.9, w * 0.9, h * 0.9] },
    { ...base, id: newId(), type: "pen", color: COLORS[5], points: [w * 0.6, h * 0.5, w * 0.65, h * 0.6, w * 0.7, h * 0.5, w * 0.75, h * 0.6] },
    { ...base, id: newId(), type: "text", color: COLORS[2], x: w * 0.1, y: h * 0.45, text: "Hello annotation", fontSize: fontSizeFor(6) },
    { ...base, id: newId(), type: "highlight", color: COLORS[2], x: w * 0.1, y: h * 0.6, width: w * 0.25, height: h * 0.08 },
    { ...base, id: newId(), type: "number", x: w * 0.5, y: h * 0.3, n: 1, radius: badgeRadiusFor(4) },
    { ...base, id: newId(), type: "blur", x: w * 0.6, y: h * 0.65, width: w * 0.25, height: h * 0.15 },
  ];
}
