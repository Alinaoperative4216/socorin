import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ipc, shareErrorMessage, type Settings } from "../lib/ipc";
import type { Anchor } from "../lib/ipc";
import { isMac } from "../lib/hotkey";
import { AnnotationStage } from "../editor/AnnotationStage";
import { Toolbar } from "../editor/Toolbar";
import { useAnnotator } from "../editor/useAnnotator";
import { sampleShapes } from "../editor/types";

/** A region of the monitor bitmap, in bitmap (physical) pixels. */
export interface Crop {
  x: number;
  y: number;
  width: number;
  height: number;
}

interface Props {
  /** The whole monitor bitmap. */
  source: HTMLCanvasElement;
  crop: Crop;
  /** CSS pixels per bitmap pixel. */
  scale: number;
  /** The selection is being moved / resized: keep out of the way. */
  hidden: boolean;
  /** True once a tool is active or something was drawn: the selection is final. */
  onLockChange: (locked: boolean) => void;
  /** Settings as of the start of the session (remembered colour / stroke). */
  settings: Settings | null;
}

const GAP = 8;

/**
 * Lark-style annotation right on the overlay: the selected region stays
 * where it is on screen, a toolbar floats next to it and the drawing tools
 * work on top of the frozen screenshot. Copy / Save end the capture.
 */
export function InPlaceEditor({ source, crop, scale, hidden, onLockChange, settings }: Props) {
  const a = useAnnotator({
    width: crop.width,
    height: crop.height,
    scale,
    // `scale` is CSS px per bitmap px, so its inverse is the bitmap's pixel ratio.
    pixelRatio: 1 / scale,
    initialTool: null,
    initialColor: settings?.annotationColor,
    initialStrokeLevel: settings?.annotationStroke,
  });
  const [busy, setBusy] = useState(false);
  const [toast, setToast] = useState<{ text: string; info?: boolean } | null>(null);
  const toastTimer = useRef<number | undefined>(undefined);
  const toolbarRef = useRef<HTMLDivElement>(null);
  const [toolbarSize, setToolbarSize] = useState({ w: 0, h: 0 });

  // The selected pixels as their own bitmap (stage background + pixelate source).
  const image = useMemo(() => {
    if (crop.x === 0 && crop.y === 0 && crop.width === source.width && crop.height === source.height) {
      return source; // whole monitor selected: no copy needed
    }
    const c = document.createElement("canvas");
    c.width = crop.width;
    c.height = crop.height;
    c.getContext("2d")!.drawImage(source, crop.x, crop.y, crop.width, crop.height, 0, 0, crop.width, crop.height);
    return c;
  }, [source, crop.x, crop.y, crop.width, crop.height]);

  const locked = a.tool !== null || a.shapes.length > 0 || a.history.canUndo;
  useEffect(() => onLockChange(locked), [locked, onLockChange]);

  // ---- toolbar placement: below the selection, else above, else inside ----
  useLayoutEffect(() => {
    const el = toolbarRef.current;
    if (!el) return;
    const measure = () => setToolbarSize({ w: el.offsetWidth, h: el.offsetHeight });
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    measure();
    return () => ro.disconnect();
  }, []);

  const box = {
    left: crop.x * scale,
    top: crop.y * scale,
    width: crop.width * scale,
    height: crop.height * scale,
  };
  const toolbarPos = useMemo(() => {
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const { w, h } = toolbarSize;
    let top = box.top + box.height + GAP;
    if (top + h > vh - GAP) {
      top = box.top - GAP - h;
      if (top < GAP) top = Math.max(GAP, box.top + box.height - GAP - h);
    }
    const left = Math.max(GAP, Math.min(box.left + box.width - w, vw - w - GAP));
    return { left, top };
  }, [box.left, box.top, box.width, box.height, toolbarSize]);

  // ---- actions ----
  const notify = useCallback((text: string, info = false) => {
    setToast({ text, info });
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), 6000);
  }, []);

  const run = useCallback(
    async (fn: () => Promise<void>) => {
      if (busy) return;
      setBusy(true);
      try {
        await fn();
      } catch (e) {
        notify(shareErrorMessage(e));
      } finally {
        setBusy(false);
      }
    },
    [busy, notify],
  );

  const { renderPng } = a;
  const copy = useCallback(
    () =>
      run(async () => {
        await ipc.copyPng(await renderPng());
        await ipc.endCapture();
      }),
    [run, renderPng],
  );
  const save = useCallback(
    () =>
      run(async () => {
        await ipc.savePng(await renderPng());
        await ipc.endCapture();
      }),
    [run, renderPng],
  );
  // Rust hides the overlays before showing the dialog and writes the file.
  const saveAs = useCallback(
    () =>
      run(async () => {
        await ipc.savePngAs(await renderPng());
      }),
    [run, renderPng],
  );
  const edit = useCallback(() => run(async () => ipc.editPng(await renderPng())), [run, renderPng]);
  // Upload & copy link: the overlay stays up ("Uploading…") until the link
  // is on the clipboard; Rust's popover announces it, so the capture ends.
  // The popover goes under the Upload button, or under the selection when
  // the shortcut was used.
  const upload = useCallback(
    (anchor?: Anchor) =>
      run(async () => {
        const png = await renderPng();
        notify("Uploading…", true);
        await ipc.uploadPng(png, anchor ?? { x: box.left, y: box.top, width: box.width, height: box.height });
        setToast(null);
        await ipc.endCapture();
      }),
    [run, renderPng, notify, box.left, box.top, box.width, box.height],
  );
  const close = useCallback(() => void ipc.endCapture(), []);

  const actions = useRef({ copy, save });
  actions.current = { copy, save };

  // ---- keyboard ----
  const { handleKey, tool } = a;
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
        else void save();
      } else if (mod && key === "e") {
        e.preventDefault();
        void edit();
      } else if (mod && e.shiftKey && key === "u") {
        e.preventDefault();
        void upload();
      } else if (e.key === "Enter") {
        e.preventDefault();
        void copy();
      } else if (e.key === "Escape") {
        e.preventDefault();
        close();
      }
    };
    // Double-click inside the selection (no tool active) finishes, like Lark.
    const onDblClick = (e: MouseEvent) => {
      if (tool !== null || e.button !== 0) return;
      const inside =
        e.clientX >= box.left && e.clientX <= box.left + box.width && e.clientY >= box.top && e.clientY <= box.top + box.height;
      if (inside) void copy();
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("dblclick", onDblClick);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("dblclick", onDblClick);
    };
  }, [handleKey, copy, save, saveAs, edit, upload, close, tool, box.left, box.top, box.width, box.height]);

  // Automated end-to-end runs (see src-tauri/src/debug.rs).
  const autoRan = useRef(false);
  useEffect(() => {
    if (autoRan.current) return;
    ipc.debugOptions().then((dbg) => {
      if (!dbg.autoAction || autoRan.current) return;
      autoRan.current = true;
      a.history.set(sampleShapes(crop.width, crop.height));
      window.setTimeout(() => void (dbg.autoAction === "copy" ? actions.current.copy() : actions.current.save()), 800);
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div className="inplace" style={{ visibility: hidden ? "hidden" : "visible" }}>
      <div className="inplace-stage" style={{ ...box, pointerEvents: a.tool === null ? "none" : "auto" }}>
        <AnnotationStage a={a} image={image} width={crop.width} height={crop.height} scale={scale} />
      </div>
      <div ref={toolbarRef} className="floating-toolbar" style={toolbarPos}>
        <Toolbar
          floating
          tool={a.tool}
          color={a.color}
          strokeLevel={a.strokeLevel}
          canUndo={a.history.canUndo}
          canRedo={a.history.canRedo}
          hasSelection={!!a.selectedId}
          busy={busy}
          onTool={a.setTool}
          onColor={a.setColor}
          onStroke={a.setStrokeLevel}
          onUndo={a.history.undo}
          onRedo={a.history.redo}
          onDelete={a.deleteSelected}
          onEdit={edit}
          onCopy={copy}
          onSave={save}
          onSaveAs={saveAs}
          onUpload={upload}
          onClose={close}
        />
        {toast && <div className={`inplace-toast ${toast.info ? "info" : ""}`}>{toast.text}</div>}
      </div>
    </div>
  );
}
