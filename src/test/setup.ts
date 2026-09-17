/**
 * Vitest setup: the UI runs in jsdom, which has no canvas, no image decoding
 * and no ResizeObserver. Everything Konva and the overlay need from those is
 * stubbed here, just enough for the code paths to run.
 */
import { afterEach, vi } from "vitest";
import { cleanup, configure } from "@testing-library/react";

// `findBy*` / `waitFor` default to 1 s, which a loaded CI machine can miss.
configure({ asyncUtilTimeout: 4000 });

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

// ---- ResizeObserver ----
const observers = new Set<MockResizeObserver>();

class MockResizeObserver {
  constructor(private cb: ResizeObserverCallback) {
    observers.add(this);
  }
  observe() {}
  unobserve() {}
  disconnect() {
    observers.delete(this);
  }
  trigger() {
    this.cb([], this as unknown as ResizeObserver);
  }
}
(globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = MockResizeObserver;

/** Fire every live ResizeObserver callback (as if the observed elements resized). */
export function triggerResizeObservers() {
  for (const o of Array.from(observers)) o.trigger();
}

// ---- ImageData ----
class MockImageData {
  readonly data: Uint8ClampedArray;
  readonly width: number;
  readonly height: number;
  readonly colorSpace = "srgb";
  constructor(a: Uint8ClampedArray | number, b: number, c?: number) {
    if (typeof a === "number") {
      this.width = a;
      this.height = b;
      this.data = new Uint8ClampedArray(a * b * 4);
    } else {
      this.width = b;
      this.height = c ?? a.length / 4 / b;
      this.data = a;
    }
  }
}
(globalThis as unknown as { ImageData: unknown }).ImageData = MockImageData;

// ---- 2D canvas context ----
/**
 * A permissive context: every method exists (a spy), a few return values
 * matter (`measureText`, `getImageData`) and properties can be set freely.
 */
function makeContext(canvas: HTMLCanvasElement): CanvasRenderingContext2D {
  const target: Record<string | symbol, unknown> = {
    canvas,
    font: "10px sans-serif",
    fillStyle: "#000000",
    strokeStyle: "#000000",
    lineWidth: 1,
    globalAlpha: 1,
    globalCompositeOperation: "source-over",
    textBaseline: "alphabetic",
    textAlign: "start",
    measureText: (text: string) => ({
      width: text.length * 7,
      actualBoundingBoxAscent: 8,
      actualBoundingBoxDescent: 2,
      fontBoundingBoxAscent: 9,
      fontBoundingBoxDescent: 3,
    }),
    getImageData: (_x: number, _y: number, w: number, h: number) => new MockImageData(Math.max(1, w | 0), Math.max(1, h | 0)),
    createImageData: (w: number, h: number) => new MockImageData(Math.max(1, w | 0), Math.max(1, h | 0)),
    createLinearGradient: () => ({ addColorStop: vi.fn() }),
    createRadialGradient: () => ({ addColorStop: vi.fn() }),
    createPattern: () => ({}),
    getTransform: () => ({ a: 1, b: 0, c: 0, d: 1, e: 0, f: 0 }),
    isPointInPath: () => false,
    isPointInStroke: () => false,
    getLineDash: () => [],
  };
  return new Proxy(target, {
    get(t, prop) {
      if (prop in t) return t[prop];
      if (typeof prop !== "string") return undefined;
      const fn = vi.fn();
      t[prop] = fn;
      return fn;
    },
    set(t, prop, value) {
      t[prop] = value;
      return true;
    },
  }) as unknown as CanvasRenderingContext2D;
}

const contexts = new WeakMap<HTMLCanvasElement, CanvasRenderingContext2D>();
HTMLCanvasElement.prototype.getContext = function getContext(this: HTMLCanvasElement) {
  let ctx = contexts.get(this);
  if (!ctx) {
    ctx = makeContext(this);
    contexts.set(this, ctx);
  }
  return ctx;
} as unknown as typeof HTMLCanvasElement.prototype.getContext;

/** A tiny valid-looking PNG payload (only the signature matters to the code under test). */
export const PNG_BYTES = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3, 4]);

HTMLCanvasElement.prototype.toBlob = function toBlob(cb: BlobCallback, type = "image/png") {
  const blob = new Blob([PNG_BYTES], { type });
  queueMicrotask(() => cb(blob));
};
HTMLCanvasElement.prototype.toDataURL = () => "data:image/png;base64,iVBORw0KGgo=";

// ---- object URLs ----
let objectUrls = 0;
URL.createObjectURL = vi.fn(() => `blob:mock/${++objectUrls}`);
URL.revokeObjectURL = vi.fn();

// ---- image decoding: `src` assignment fires load (or error for "fail" URLs) ----
export const mockImage = { width: 800, height: 600 };

Object.defineProperty(HTMLImageElement.prototype, "src", {
  configurable: true,
  get(this: HTMLImageElement) {
    return this.getAttribute("src") ?? "";
  },
  set(this: HTMLImageElement, value: string) {
    this.setAttribute("src", value);
    queueMicrotask(() => {
      if (value.includes("fail")) this.dispatchEvent(new Event("error"));
      else this.dispatchEvent(new Event("load"));
    });
  },
});
Object.defineProperty(HTMLImageElement.prototype, "naturalWidth", {
  configurable: true,
  get: () => mockImage.width,
});
Object.defineProperty(HTMLImageElement.prototype, "naturalHeight", {
  configurable: true,
  get: () => mockImage.height,
});

// ---- misc browser bits ----
if (!window.requestAnimationFrame) {
  window.requestAnimationFrame = (cb) => window.setTimeout(() => cb(performance.now()), 16);
  window.cancelAnimationFrame = (id) => window.clearTimeout(id);
}
