import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { ipc, shareErrorMessage, type Settings } from "../lib/ipc";
import type { Anchor } from "../lib/ipc";
import { isMac } from "../lib/hotkey";
import { AnnotationStage } from "../editor/AnnotationStage";
import { Toolbar, TOOLS } from "../editor/Toolbar";
import { useAnnotator } from "../editor/useAnnotator";
import { sampleShapes } from "../editor/types";

const ZOOM_STEPS = [0.1, 0.15, 0.25, 0.33, 0.5, 0.67, 0.75, 1, 1.25, 1.5, 2, 3, 4];
type Toast = { text: string; error?: boolean } | null;

async function loadPendingImage(): Promise<HTMLImageElement> {
  const buffer = await ipc.pendingImage();
  const url = URL.createObjectURL(new Blob([buffer], { type: "image/png" }));
  const img = new Image();
  await new Promise<void>((resolve, reject) => {
    img.onload = () => resolve();
    img.onerror = () => reject(new Error("cannot decode screenshot"));
    img.src = url;
  });
  return img;
}

/** Stand-alone editor window (zoomable), fed by `pending_image`. */
export function Editor() {
  const [image, setImage] = useState<HTMLImageElement | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [zoom, setZoom] = useState(1);
  const [viewport, setViewport] = useState({ w: 0, h: 0 });
  const [busy, setBusy] = useState(false);
  const [toast, setToast] = useState<Toast>(null);
  // The image is a capture at the screen's pixel ratio, so stroke presets
  // (logical px) scale by the same factor to look like they did on screen.
  const a = useAnnotator({
    width: image?.naturalWidth ?? 1,
    height: image?.naturalHeight ?? 1,
    scale: zoom,
    pixelRatio: window.devicePixelRatio || 1,
    initialColor: settings?.annotationColor,
    initialStrokeLevel: settings?.annotationStroke,
  });

  const viewportRef = useRef<HTMLDivElement>(null);
  const toastTimer = useRef<number | undefined>(undefined);

  const notify = useCallback((text: string, error = false) => {
    setToast({ text, error });
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), error ? 6000 : 2500);
  }, []);

  const fitZoom = useCallback(
    (img: HTMLImageElement | null = image, vp = viewport) => {
      if (!img || vp.w === 0) return 1;
      const z = Math.min(1, (vp.w - 32) / img.naturalWidth, (vp.h - 32) / img.naturalHeight);
      return Math.max(0.05, Math.floor(z * 100) / 100);
    },
    [image, viewport],
  );

  // ---- load image (initially and when a new capture arrives) ----
  const load = useCallback(async () => {
    try {
      const img = await loadPendingImage();
      setImage((old) => {
        if (old) URL.revokeObjectURL(old.src);
        return img;
      });
      setLoadError(null);
      a.reset();
      if (!a.tool) a.setTool("arrow");
      void getCurrentWindow().setTitle(`Socorin – ${img.naturalWidth} × ${img.naturalHeight}`);
    } catch (e) {
      setLoadError(String(e));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [a.reset]);

  // Automated end-to-end runs (see src-tauri/src/debug.rs): add one of each
  // annotation and run the requested action.
  const autoRan = useRef(false);
  const actionsRef = useRef({ copy: () => Promise.resolve(), quickSave: () => Promise.resolve() });
  useEffect(() => {
    if (!image || autoRan.current) return;
    ipc.debugOptions().then((dbg) => {
      if (!dbg.autoAction || autoRan.current) return;
      autoRan.current = true;
      a.history.set(sampleShapes(image.naturalWidth, image.naturalHeight));
      window.setTimeout(() => void (dbg.autoAction === "copy" ? actionsRef.current.copy() : actionsRef.current.quickSave()), 800);
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [image]);

  useEffect(() => {
    void load();
    ipc.getSettings().then(setSettings).catch(console.error);
    const unlisten = listen("editor:reload", () => void load());
    return () => {
      unlisten.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ---- viewport size ----
  useLayoutEffect(() => {
    const el = viewportRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setViewport({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    setViewport({ w: el.clientWidth, h: el.clientHeight });
    return () => ro.disconnect();
  }, [image]);

  // Fit once per image load.
  const fittedFor = useRef<HTMLImageElement | null>(null);
  useEffect(() => {
    if (image && viewport.w > 0 && fittedFor.current !== image) {
      fittedFor.current = image;
      setZoom(fitZoom(image, viewport));
    }
  }, [image, viewport, fitZoom]);

  // ---- actions ----
  const run = useCallback(
    async (fn: () => Promise<void>) => {
      if (busy) return;
      setBusy(true);
      try {
        await fn();
      } catch (e) {
        notify(shareErrorMessage(e), true);
      } finally {
        setBusy(false);
      }
    },
    [busy, notify],
  );

  const copy = useCallback(
    () =>
      run(async () => {
        await ipc.copyPng(await a.renderPng());
        notify("Copied to clipboard");
      }),
    [run, a.renderPng, notify],
  );

  const quickSave = useCallback(
    () =>
      run(async () => {
        const path = await ipc.savePng(await a.renderPng());
        notify(`Saved to ${path}`);
      }),
    [run, a.renderPng, notify],
  );

  // The dialog runs in Rust (see save_png_as): the page never names a path.
  const saveAs = useCallback(
    () =>
      run(async () => {
        const saved = await ipc.savePngAs(await a.renderPng());
        if (saved) notify(`Saved to ${saved}`);
      }),
    [run, a.renderPng, notify],
  );

  // Upload & copy link: Rust copies the link and shows the popover; the
  // status bar says so as well while this window has the eye.
  // `anchor`: the Upload button, where the popover goes (none from the shortcut).
  const upload = useCallback(
    (anchor?: Anchor) =>
      run(async () => {
        const png = await a.renderPng();
        notify("Uploading…");
        await ipc.uploadPng(png, anchor);
        notify("Link copied");
      }),
    [run, a.renderPng, notify],
  );

  actionsRef.current = { copy, quickSave };

  const close = useCallback(() => void getCurrentWindow().close(), []);

  const changeZoom = useCallback((delta: number) => {
    setZoom((z) => {
      const idx = ZOOM_STEPS.findIndex((s) => s >= z - 0.001);
      const next = delta > 0 ? ZOOM_STEPS[Math.min(ZOOM_STEPS.length - 1, (idx < 0 ? ZOOM_STEPS.length - 1 : idx) + 1)] : ZOOM_STEPS[Math.max(0, (idx < 0 ? ZOOM_STEPS.length : idx) - 1)];
      return next ?? z;
    });
  }, []);

  // ---- keyboard shortcuts ----
  const { handleKey } = a;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (handleKey(e)) return;
      const mod = isMac ? e.metaKey : e.ctrlKey;
      const key = e.key.toLowerCase();
      if (mod && key === "c") {
        e.preventDefault();
        void copy();
      } else if (mod && key === "s") {
        e.preventDefault();
        if (e.shiftKey) void saveAs();
        else void quickSave();
      } else if (mod && e.shiftKey && key === "u") {
        e.preventDefault();
        void upload();
      } else if (mod && (key === "=" || key === "+")) {
        e.preventDefault();
        changeZoom(1);
      } else if (mod && key === "-") {
        e.preventDefault();
        changeZoom(-1);
      } else if (mod && key === "0") {
        e.preventDefault();
        setZoom(fitZoom());
      } else if (e.key === "Escape") {
        e.preventDefault();
        close();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [handleKey, copy, quickSave, saveAs, upload, changeZoom, fitZoom, close]);

  // Ctrl/⌘ + wheel zoom.
  useEffect(() => {
    const el = viewportRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (!(isMac ? e.metaKey : e.ctrlKey)) return;
      e.preventDefault();
      changeZoom(e.deltaY < 0 ? 1 : -1);
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [changeZoom, image]);

  // ---- render ----
  if (loadError || !image) {
    return (
      <div className="editor">
        <div className="empty-state">
          <p>{loadError ? "No screenshot to edit yet." : "Loading…"}</p>
          {loadError && (
            <button type="button" onClick={() => ipc.startCapture()}>
              Capture a region
            </button>
          )}
        </div>
      </div>
    );
  }

  return (
    <div className="editor">
      <Toolbar
        tool={a.tool}
        color={a.color}
        strokeLevel={a.strokeLevel}
        zoom={zoom}
        canUndo={a.history.canUndo}
        canRedo={a.history.canRedo}
        hasSelection={!!a.selectedId}
        busy={busy}
        onTool={(t) => a.setTool(t ?? "select")}
        onColor={a.setColor}
        onStroke={a.setStrokeLevel}
        onUndo={a.history.undo}
        onRedo={a.history.redo}
        onDelete={a.deleteSelected}
        onZoom={changeZoom}
        onFit={() => setZoom(fitZoom())}
        onCopy={copy}
        onSave={quickSave}
        onSaveAs={saveAs}
        onUpload={upload}
        onClose={close}
      />

      <div className="canvas-viewport" ref={viewportRef}>
        <AnnotationStage a={a} image={image} width={image.naturalWidth} height={image.naturalHeight} scale={zoom} />
      </div>

      <div className="statusbar">
        <span>
          {image.naturalWidth} × {image.naturalHeight}
        </span>
        <span>{TOOLS.find((t) => t.id === a.tool)?.label}</span>
        {a.tool === "number" && <span>next: {a.shapes.filter((s) => s.type === "number").length + 1}</span>}
        {toast && <span className={`toast ${toast.error ? "error" : ""}`}>{toast.text}</span>}
      </div>
    </div>
  );
}
