import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { ipc, type Bar, type CaptureMode, type MonitorInfo, type Settings } from "../lib/ipc";
import type { Crop } from "./InPlaceEditor";
import { RecordBar } from "./RecordBar";

// Konva and the toolbar are only needed once a region is selected.
const InPlaceEditor = lazy(() => import("./InPlaceEditor").then((m) => ({ default: m.InPlaceEditor })));

interface Point { x: number; y: number }
interface Drag { start: Point; end: Point }
interface Rect { x: number; y: number; w: number; h: number }

type Phase = "idle" | "select" | "annotate";
type Handle = "nw" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w";
interface Adjust { mode: "move" | "resize"; handle?: Handle; start: Point; rect: Rect }

const MIN_SIZE = 3;
const HANDLE = 8;
const HANDLE_CURSOR: Record<Handle, string> = {
  nw: "nwse-resize", se: "nwse-resize", ne: "nesw-resize", sw: "nesw-resize",
  n: "ns-resize", s: "ns-resize", e: "ew-resize", w: "ew-resize",
};

function normalize(d: Drag): Rect {
  const x = Math.min(d.start.x, d.end.x);
  const y = Math.min(d.start.y, d.end.y);
  return { x, y, w: Math.abs(d.end.x - d.start.x), h: Math.abs(d.end.y - d.start.y) };
}

function handlePoints(r: Rect): Record<Handle, Point> {
  const cx = r.x + r.w / 2;
  const cy = r.y + r.h / 2;
  return {
    nw: { x: r.x, y: r.y }, n: { x: cx, y: r.y }, ne: { x: r.x + r.w, y: r.y },
    e: { x: r.x + r.w, y: cy }, se: { x: r.x + r.w, y: r.y + r.h }, s: { x: cx, y: r.y + r.h },
    sw: { x: r.x, y: r.y + r.h }, w: { x: r.x, y: cy },
  };
}

function hitHandle(r: Rect, p: Point): Handle | null {
  const tolerance = HANDLE;
  for (const [id, hp] of Object.entries(handlePoints(r)) as [Handle, Point][]) {
    if (Math.abs(p.x - hp.x) <= tolerance && Math.abs(p.y - hp.y) <= tolerance) return id;
  }
  return null;
}

function inside(r: Rect, p: Point): boolean {
  return p.x >= r.x && p.x <= r.x + r.w && p.y >= r.y && p.y <= r.y + r.h;
}

/** Apply a move / resize gesture to the selection, keeping it on screen. */
function applyAdjust(adj: Adjust, p: Point): Rect {
  const W = window.innerWidth;
  const H = window.innerHeight;
  const dx = p.x - adj.start.x;
  const dy = p.y - adj.start.y;
  const r = adj.rect;
  if (adj.mode === "move") {
    return {
      x: Math.max(0, Math.min(W - r.w, r.x + dx)),
      y: Math.max(0, Math.min(H - r.h, r.y + dy)),
      w: r.w,
      h: r.h,
    };
  }
  const h = adj.handle!;
  let x1 = r.x;
  let y1 = r.y;
  let x2 = r.x + r.w;
  let y2 = r.y + r.h;
  if (h.includes("w")) x1 = Math.max(0, Math.min(W, x1 + dx));
  if (h.includes("e")) x2 = Math.max(0, Math.min(W, x2 + dx));
  if (h.startsWith("n")) y1 = Math.max(0, Math.min(H, y1 + dy));
  if (h.startsWith("s")) y2 = Math.max(0, Math.min(H, y2 + dy));
  const out = normalize({ start: { x: x1, y: y1 }, end: { x: x2, y: y2 } });
  return { ...out, w: Math.max(MIN_SIZE, out.w), h: Math.max(MIN_SIZE, out.h) };
}

function drawLabel(ctx: CanvasRenderingContext2D, text: string, x: number, y: number) {
  ctx.font = "12px -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif";
  const padX = 6;
  const metrics = ctx.measureText(text);
  const w = metrics.width + padX * 2;
  const h = 20;
  ctx.fillStyle = "rgba(20, 20, 20, 0.85)";
  ctx.beginPath();
  ctx.roundRect(x, y, w, h, 4);
  ctx.fill();
  ctx.fillStyle = "#fff";
  ctx.textBaseline = "middle";
  ctx.fillText(text, x + padX, y + h / 2);
}

/**
 * Full-screen selection overlay, one instance per monitor. The window lives
 * hidden between captures; Rust sends `capture:start` with the monitor info,
 * we fetch the frozen screenshot, paint it dimmed and report `overlay_ready`
 * so the window gets shown.
 *
 * The user drags a rectangle. With "annotate" as the after-capture setting
 * the selection then stays on screen (movable / resizable until something is
 * drawn) and the in-place editor takes over: the picture never moves, the
 * toolbar floats next to it. Otherwise the region is reported to Rust right
 * away (copy / save). `capture:reset` returns us to the idle state.
 */
export function Overlay({ monitorId }: { monitorId: number }) {
  const rootRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const sourceRef = useRef<HTMLCanvasElement | null>(null);
  const infoRef = useRef<MonitorInfo | null>(null);
  const dragRef = useRef<Drag | null>(null);
  const cursorRef = useRef<Point | null>(null);
  const finishedRef = useRef(false);
  const frameRef = useRef(0);
  const sessionRef = useRef(0);
  const phaseRef = useRef<Phase>("idle");
  const rectRef = useRef<Rect | null>(null);
  const adjustRef = useRef<Adjust | null>(null);
  const lockedRef = useRef(false); // annotations exist: selection is final
  const peerLockRef = useRef(false); // another monitor's overlay is annotating
  const inPlaceRef = useRef(true);
  const modeRef = useRef<CaptureMode>("screenshot");
  const autoSelectRef = useRef<[number, number, number, number] | null>(null);
  const diagRef = useRef({ debug: false, sawMouseMove: false, sawCursorEvent: false });
  const [ready, setReady] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [phase, setPhase] = useState<Phase>("idle");
  const [crop, setCrop] = useState<Crop | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [mode, setMode] = useState<CaptureMode>("screenshot");
  const [adjusting, setAdjusting] = useState(false);
  const [peerLocked, setPeerLocked] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const imageScale = useCallback(() => {
    const info = infoRef.current;
    if (!info) return { sx: 1, sy: 1 };
    return { sx: info.imageWidth / window.innerWidth, sy: info.imageHeight / window.innerHeight };
  }, []);

  /** The bitmap region (physical pixels) under a CSS rectangle. */
  const cropOf = useCallback(
    (rect: Rect): Crop => {
      const { sx, sy } = imageScale();
      const info = infoRef.current!;
      const x = Math.max(0, Math.round(rect.x * sx));
      const y = Math.max(0, Math.round(rect.y * sy));
      return {
        x,
        y,
        width: Math.min(info.imageWidth - x, Math.max(1, Math.round(rect.w * sx))),
        height: Math.min(info.imageHeight - y, Math.max(1, Math.round(rect.h * sy))),
      };
    },
    [imageScale],
  );

  const setCursor = useCallback((cursor: string) => {
    const el = rootRef.current;
    if (el && el.style.cursor !== cursor) el.style.cursor = cursor;
  }, []);

  const draw = useCallback(() => {
    frameRef.current = 0;
    const canvas = canvasRef.current;
    const source = sourceRef.current;
    if (!canvas || !source) return;
    const dpr = window.devicePixelRatio || 1;
    const w = window.innerWidth;
    const h = window.innerHeight;
    const pw = Math.round(w * dpr);
    const ph = Math.round(h * dpr);
    if (canvas.width !== pw || canvas.height !== ph) {
      canvas.width = pw;
      canvas.height = ph;
      canvas.style.width = `${w}px`;
      canvas.style.height = `${h}px`;
    }
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.drawImage(source, 0, 0, w, h);

    const { sx, sy } = imageScale();
    ctx.fillStyle = "rgba(0, 0, 0, 0.45)";

    if (phaseRef.current === "annotate" && rectRef.current) {
      // Selection is final: dim the rest, frame it, show handles while it
      // can still be adjusted. The in-place editor draws on top of it.
      const r = rectRef.current;
      ctx.beginPath();
      ctx.rect(0, 0, w, h);
      ctx.rect(r.x, r.y, r.w, r.h);
      ctx.fill("evenodd");
      ctx.strokeStyle = "rgba(255,255,255,0.95)";
      ctx.lineWidth = 1;
      ctx.strokeRect(r.x - 0.5, r.y - 0.5, r.w + 1, r.h + 1);
      const label = `${Math.round(r.w * sx)} × ${Math.round(r.h * sy)}`;
      const ly = r.y >= 26 ? r.y - 24 : r.y + 4;
      drawLabel(ctx, label, r.x, Math.min(ly, h - 24));
      if (!lockedRef.current) {
        for (const p of Object.values(handlePoints(r))) {
          ctx.fillStyle = "#fff";
          ctx.strokeStyle = "rgba(0,0,0,0.6)";
          ctx.beginPath();
          ctx.rect(p.x - HANDLE / 2, p.y - HANDLE / 2, HANDLE, HANDLE);
          ctx.fill();
          ctx.stroke();
        }
      }
      return;
    }

    const rect = dragRef.current ? normalize(dragRef.current) : null;
    if (rect && rect.w > 0 && rect.h > 0) {
      ctx.beginPath();
      ctx.rect(0, 0, w, h);
      ctx.rect(rect.x, rect.y, rect.w, rect.h);
      ctx.fill("evenodd");
      ctx.strokeStyle = "rgba(255,255,255,0.95)";
      ctx.lineWidth = 1;
      ctx.strokeRect(rect.x + 0.5, rect.y + 0.5, Math.max(0, rect.w - 1), Math.max(0, rect.h - 1));
      const label = `${Math.round(rect.w * sx)} × ${Math.round(rect.h * sy)}`;
      const ly = rect.y >= 26 ? rect.y - 24 : rect.y + rect.h + 4;
      drawLabel(ctx, label, rect.x, Math.min(ly, h - 24));
    } else {
      ctx.fillRect(0, 0, w, h);
      const cur = cursorRef.current;
      if (cur && !peerLockRef.current) {
        ctx.strokeStyle = "rgba(255,255,255,0.6)";
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.moveTo(cur.x + 0.5, 0);
        ctx.lineTo(cur.x + 0.5, h);
        ctx.moveTo(0, cur.y + 0.5);
        ctx.lineTo(w, cur.y + 0.5);
        ctx.stroke();
        const label = `${Math.round(cur.x * sx)}, ${Math.round(cur.y * sy)}`;
        const lx = cur.x + 14 + 90 > w ? cur.x - 90 : cur.x + 14;
        const ly = cur.y + 14 + 20 > h ? cur.y - 26 : cur.y + 14;
        drawLabel(ctx, label, lx, ly);
      }
    }
  }, [imageScale]);

  const scheduleDraw = useCallback(() => {
    if (!frameRef.current) frameRef.current = requestAnimationFrame(draw);
  }, [draw]);

  const cancel = useCallback(() => {
    if (finishedRef.current) return;
    finishedRef.current = true;
    void ipc.cancelCapture();
  }, []);

  /** Immediate modes (copy / save): hand the region to Rust. */
  const finish = useCallback(
    (rect: Rect) => {
      if (finishedRef.current) return;
      finishedRef.current = true;
      const c = cropOf(rect);
      ipc.finishRegion({ monitorId, ...c }).catch((e) => {
        console.error(e);
        finishedRef.current = false;
        setError(String(e));
      });
    },
    [cropOf, monitorId],
  );

  /** Make `rect` the final selection (state only; nothing is drawn or told to Rust). */
  const applySelection = useCallback(
    (rect: Rect) => {
      finishedRef.current = true;
      dragRef.current = null;
      rectRef.current = rect;
      lockedRef.current = false;
      phaseRef.current = "annotate";
      setDragging(false);
      setPhase("annotate");
      setCrop(cropOf(rect));
      setCursor("default");
      if (diagRef.current.debug) void ipc.debugLog(`overlay ${monitorId} annotate ${JSON.stringify(rect)}`);
    },
    [cropOf, monitorId, setCursor],
  );

  /** Annotate mode: keep the selection on screen and start the editor. */
  const enterAnnotate = useCallback(
    (rect: Rect) => {
      if (finishedRef.current) return;
      applySelection(rect);
      void ipc.beginAnnotation(monitorId);
      scheduleDraw();
    },
    [applySelection, monitorId, scheduleDraw],
  );

  // Record mode always keeps the selection on screen (adjustable) and
  // shows the Record bar instead of the annotation tools.
  const select = useCallback(
    (rect: Rect) => (inPlaceRef.current || modeRef.current === "record" ? enterAnnotate(rect) : finish(rect)),
    [enterAnnotate, finish],
  );

  /** Record: hand Rust the area and where the bar is (the recording bar goes there). */
  const startRecording = useCallback(
    (bar: Bar) => {
      if (finishedRef.current && phaseRef.current !== "annotate") return;
      const rect = rectRef.current;
      if (!rect) return;
      const c = cropOf(rect);
      if (diagRef.current.debug) void ipc.debugLog(`overlay ${monitorId} record ${JSON.stringify(c)} bar ${JSON.stringify(bar)}`);
      ipc.startRecording({ monitorId, ...c }, bar).catch((e) => setError(String(e)));
    },
    [cropOf, monitorId],
  );

  const cancelRecording = useCallback(() => void ipc.cancelCapture(), []);

  const onLockChange = useCallback(
    (locked: boolean) => {
      if (lockedRef.current === locked) return;
      lockedRef.current = locked;
      if (locked) setCursor("default");
      scheduleDraw();
    },
    [scheduleDraw, setCursor],
  );

  /** Back to idle: drop the bitmap so a hidden overlay costs little memory. */
  const reset = useCallback(() => {
    if (diagRef.current.debug) void ipc.debugLog(`overlay ${monitorId} reset`);
    diagRef.current.sawMouseMove = false;
    diagRef.current.sawCursorEvent = false;
    dragRef.current = null;
    cursorRef.current = null;
    finishedRef.current = false;
    sourceRef.current = null;
    infoRef.current = null;
    rectRef.current = null;
    adjustRef.current = null;
    lockedRef.current = false;
    peerLockRef.current = false;
    phaseRef.current = "idle";
    modeRef.current = "screenshot";
    const canvas = canvasRef.current;
    if (canvas) {
      canvas.width = 0;
      canvas.height = 0;
    }
    setCursor("");
    setDragging(false);
    setAdjusting(false);
    setPeerLocked(false);
    setCrop(null);
    setPhase("idle");
    setMode("screenshot");
    setReady(false);
    setError(null);
  }, [monitorId, setCursor]);

  /** A capture session started: fetch the frozen screenshot and paint it. */
  const start = useCallback(
    async (info: MonitorInfo) => {
      if (info.id !== monitorId || info.session === sessionRef.current) return;
      sessionRef.current = info.session;
      dragRef.current = null;
      rectRef.current = null;
      adjustRef.current = null;
      finishedRef.current = false;
      lockedRef.current = false;
      peerLockRef.current = false;
      phaseRef.current = "select";
      modeRef.current = info.mode;
      diagRef.current.sawMouseMove = false;
      diagRef.current.sawCursorEvent = false;
      setCursor("");
      setDragging(false);
      setAdjusting(false);
      setPeerLocked(false);
      setCrop(null);
      setPhase("select");
      setMode(info.mode);
      setError(null);
      try {
        const [buffer, settings] = await Promise.all([
          ipc.overlayPixels(monitorId),
          ipc.getSettings().catch(() => null),
        ]);
        if (sessionRef.current !== info.session) return; // superseded
        inPlaceRef.current = !settings || settings.afterCapture === "editor";
        setSettings(settings);
        const source = document.createElement("canvas");
        source.width = info.imageWidth;
        source.height = info.imageHeight;
        const data = new ImageData(new Uint8ClampedArray(buffer), info.imageWidth, info.imageHeight);
        source.getContext("2d")!.putImageData(data, 0, 0);
        infoRef.current = info;
        sourceRef.current = source;
        // Full-screen capture: the whole monitor is the selection from the
        // very first frame, so the window never shows the dimmed crosshair
        // view (the editor mounting afterwards is heavy on a 4K screen and
        // would keep that frame on screen for a noticeable moment).
        const fullRect = { x: 0, y: 0, w: window.innerWidth, h: window.innerHeight };
        const preselected = info.preselectFull && inPlaceRef.current;
        if (preselected) applySelection(fullRect);
        setReady(true);
        draw();
        if (diagRef.current.debug) {
          void ipc.debugLog(
            `overlay ${monitorId} painted ${window.innerWidth}x${window.innerHeight} css px, phase ${phaseRef.current}, cursor ${cursorRef.current ? "known" : "unknown"}`,
          );
        }
        await ipc.overlayReady(monitorId);
        if (preselected) {
          // Now that the window is visible: focus it, lock the other overlays.
          void ipc.beginAnnotation(monitorId);
        } else if (info.preselectFull) {
          select(fullRect);
        } else if (autoSelectRef.current && info.primary) {
          // Automated end-to-end runs (see src-tauri/src/debug.rs).
          const [x, y, w, h] = autoSelectRef.current;
          setTimeout(() => select({ x, y, w, h }), 400);
        }
      } catch (e) {
        console.error(e);
        setError(String(e));
        setTimeout(() => void ipc.cancelCapture(), 1500);
      }
    },
    [monitorId, draw, select, applySelection, setCursor],
  );

  useEffect(() => {
    let disposed = false;
    // Listen as *this* webview window: Rust targets each overlay separately
    // and a plain `listen()` would also receive the other monitors' events.
    const self = getCurrentWebviewWindow();
    const unlisteners = [
      self.listen<MonitorInfo>("capture:start", (e) => void start(e.payload)),
      self.listen("capture:reset", () => reset()),
      // Another monitor's overlay owns the selection now: just stay dimmed.
      self.listen("capture:lock", () => {
        peerLockRef.current = true;
        cursorRef.current = null;
        dragRef.current = null;
        setPeerLocked(true);
        setDragging(false);
        scheduleDraw();
      }),
      // Native cursor tracking from Rust: keeps the crosshair alive even when
      // the OS did not make this window the key window (no DOM mousemove).
      self.listen<Point>("cursor:move", (e) => {
        if (dragRef.current || phaseRef.current === "annotate") return;
        cursorRef.current = e.payload;
        if (diagRef.current.debug && !diagRef.current.sawCursorEvent) {
          diagRef.current.sawCursorEvent = true;
          void ipc.debugLog(`overlay ${monitorId} first cursor:move ${Math.round(e.payload.x)},${Math.round(e.payload.y)}`);
        }
        scheduleDraw();
      }),
    ];
    ipc.debugOptions()
      .then((d) => {
        if (disposed) return;
        diagRef.current.debug = d.enabled;
        if (d.autoSelect) autoSelectRef.current = d.autoSelect;
      })
      .catch(() => {});
    // The window may have been created in the middle of a session (a monitor
    // was plugged in, or this is the very first capture): pick it up.
    ipc.overlayInfo(monitorId)
      .then((info) => {
        if (!disposed) void start(info);
      })
      .catch(() => {
        /* idle */
      });
    // Warm the editor chunk while idle so the first selection is instant.
    const warm = window.setTimeout(() => void import("./InPlaceEditor"), 1500);
    return () => {
      disposed = true;
      window.clearTimeout(warm);
      unlisteners.forEach((p) => p.then((f) => f()));
    };
  }, [monitorId, start, reset, scheduleDraw]);

  // Mouse / keyboard handling.
  useEffect(() => {
    const isEditorUi = (t: EventTarget | null) => t instanceof Element && !!t.closest(".inplace");

    const onMouseDown = (e: MouseEvent) => {
      if (!ready || peerLockRef.current) return;
      if (phaseRef.current === "annotate") {
        // Move / resize the selection until something is drawn; the editor
        // (stage, toolbar) handles its own clicks.
        if (isEditorUi(e.target) || e.button !== 0 || lockedRef.current || !rectRef.current) return;
        const p = { x: e.clientX, y: e.clientY };
        const r = rectRef.current;
        const handle = hitHandle(r, p);
        if (handle) adjustRef.current = { mode: "resize", handle, start: p, rect: r };
        else if (inside(r, p)) adjustRef.current = { mode: "move", start: p, rect: r };
        else return;
        e.preventDefault();
        setAdjusting(true);
        return;
      }
      if (finishedRef.current) return;
      if (e.button === 2) {
        e.preventDefault();
        cancel();
        return;
      }
      if (e.button !== 0) return;
      dragRef.current = { start: { x: e.clientX, y: e.clientY }, end: { x: e.clientX, y: e.clientY } };
      setDragging(true);
      scheduleDraw();
    };
    const onMouseMove = (e: MouseEvent) => {
      if (!ready || peerLockRef.current) return;
      const p = {
        x: Math.max(0, Math.min(window.innerWidth, e.clientX)),
        y: Math.max(0, Math.min(window.innerHeight, e.clientY)),
      };
      if (phaseRef.current === "annotate") {
        const adj = adjustRef.current;
        if (adj) {
          rectRef.current = applyAdjust(adj, p);
          scheduleDraw();
        } else if (!lockedRef.current && rectRef.current && !isEditorUi(e.target)) {
          const handle = hitHandle(rectRef.current, p);
          setCursor(handle ? HANDLE_CURSOR[handle] : inside(rectRef.current, p) ? "move" : "default");
        }
        return;
      }
      if (diagRef.current.debug && !diagRef.current.sawMouseMove) {
        diagRef.current.sawMouseMove = true;
        void ipc.debugLog(`overlay ${monitorId} first DOM mousemove ${e.clientX},${e.clientY}`);
      }
      cursorRef.current = { x: e.clientX, y: e.clientY };
      if (dragRef.current) dragRef.current.end = p;
      scheduleDraw();
    };
    const onMouseUp = (e: MouseEvent) => {
      if (e.button !== 0) return;
      if (phaseRef.current === "annotate") {
        if (adjustRef.current && rectRef.current) {
          adjustRef.current = null;
          setCrop(cropOf(rectRef.current));
          setAdjusting(false);
          scheduleDraw();
        }
        return;
      }
      if (!dragRef.current) return;
      const rect = normalize(dragRef.current);
      if (rect.w >= MIN_SIZE && rect.h >= MIN_SIZE) {
        select(rect);
      } else {
        dragRef.current = null;
        setDragging(false);
        scheduleDraw();
      }
    };
    const onKeyDown = (e: KeyboardEvent) => {
      // While annotating the in-place editor owns the keyboard.
      if (phaseRef.current === "annotate") return;
      if (e.key === "Escape" && ready) {
        e.preventDefault();
        if (diagRef.current.debug) void ipc.debugLog(`overlay ${monitorId} DOM Escape`);
        cancel();
      }
    };
    const onMouseEnter = () => {
      // Keyboard focus follows the mouse so Esc works on every monitor.
      if (ready && !peerLockRef.current) void getCurrentWindow().setFocus();
    };
    const onContextMenu = (e: Event) => e.preventDefault();
    const onResize = () => scheduleDraw();

    window.addEventListener("mousedown", onMouseDown);
    window.addEventListener("mousemove", onMouseMove);
    window.addEventListener("mouseup", onMouseUp);
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("contextmenu", onContextMenu);
    window.addEventListener("resize", onResize);
    document.documentElement.addEventListener("mouseenter", onMouseEnter);
    return () => {
      window.removeEventListener("mousedown", onMouseDown);
      window.removeEventListener("mousemove", onMouseMove);
      window.removeEventListener("mouseup", onMouseUp);
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("contextmenu", onContextMenu);
      window.removeEventListener("resize", onResize);
      document.documentElement.removeEventListener("mouseenter", onMouseEnter);
    };
  }, [ready, cancel, select, cropOf, scheduleDraw, setCursor, monitorId]);

  const { sx } = imageScale();

  return (
    <div className="overlay" ref={rootRef}>
      <canvas ref={canvasRef} className="overlay-canvas" />
      {ready && phase === "select" && !dragging && !peerLocked && (
        <div className="overlay-hint">
          {mode === "record" ? "Drag to select the area to record" : "Drag to select an area"} &nbsp;·&nbsp; <kbd>Esc</kbd> to
          cancel
        </div>
      )}
      {phase === "annotate" && mode === "screenshot" && crop && sourceRef.current && (
        <Suspense fallback={null}>
          <InPlaceEditor
            source={sourceRef.current}
            crop={crop}
            scale={1 / sx}
            hidden={adjusting}
            onLockChange={onLockChange}
            settings={settings}
          />
        </Suspense>
      )}
      {phase === "annotate" && mode === "record" && crop && (
        <RecordBar crop={crop} scale={1 / sx} hidden={adjusting} onStart={startRecording} onCancel={cancelRecording} />
      )}
      {error && <div className="overlay-hint overlay-error">{error}</div>}
    </div>
  );
}
