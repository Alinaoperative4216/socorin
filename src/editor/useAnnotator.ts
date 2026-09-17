import { useCallback, useEffect, useRef, useState } from "react";
import Konva from "konva";
import type { KonvaEventObject } from "konva/lib/Node";
import { isMac } from "../lib/hotkey";
import { ipc, type Settings } from "../lib/ipc";
import { TOOLS } from "./Toolbar";
import { useHistory } from "./useHistory";
import {
  BOX_TOOLS,
  LINE_TOOLS,
  COLORS,
  DEFAULT_STROKE_LEVEL,
  badgeRadiusFor,
  clampStrokeLevel,
  fontSizeFor,
  newId,
  strokeWidthFor,
  type BoxShape,
  type LineShape,
  type PenShape,
  type Shape,
  type TextShape,
  type ToolId,
} from "./types";

export interface AnnotatorOptions {
  /** Drawing surface size in image (export) pixels. */
  width: number;
  height: number;
  /** Stage pixels per image pixel (the zoom). */
  scale: number;
  /** Image pixels per logical (screen) pixel; stroke presets are logical. */
  pixelRatio?: number;
  /** `null` = no drawing tool active (the in-place editor starts that way). */
  initialTool?: ToolId | null;
  /** Remembered style from the settings; applied whenever they change. */
  initialColor?: string;
  initialStrokeLevel?: number;
}

/** Persist style changes a little after the last one (colour pickers fire continuously). */
function useStylePersistence() {
  const pending = useRef<Partial<Settings>>({});
  const timer = useRef<number | undefined>(undefined);
  return useCallback((patch: Partial<Settings>) => {
    pending.current = { ...pending.current, ...patch };
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => {
      const p = pending.current;
      pending.current = {};
      ipc.updateSettings(p).catch(console.error);
    }, 400);
  }, []);
}

/**
 * Annotation state shared by the editor window and the in-place editor:
 * shapes with undo/redo, the active tool, selection, text editing, the
 * drawing gesture on the stage and the PNG export.
 */
export function useAnnotator({
  width,
  height,
  scale,
  pixelRatio = 1,
  initialTool = "arrow",
  initialColor,
  initialStrokeLevel,
}: AnnotatorOptions) {
  const history = useHistory<Shape[]>([]);
  const shapes = history.value;
  const [draft, setDraft] = useState<Shape | null>(null);
  const [tool, setTool] = useState<ToolId | null>(initialTool);
  const [color, setColorState] = useState(initialColor || COLORS[0]);
  const [strokeLevel, setStrokeLevelState] = useState(clampStrokeLevel(initialStrokeLevel ?? DEFAULT_STROKE_LEVEL));
  const strokeWidth = strokeWidthFor(strokeLevel, pixelRatio);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);

  const stageRef = useRef<Konva.Stage>(null);
  const layerRef = useRef<Konva.Layer>(null);
  const trRef = useRef<Konva.Transformer>(null);
  const drawingRef = useRef(false);
  const boundsRef = useRef({ width, height, scale });
  boundsRef.current = { width, height, scale };

  // ---- style: user changes are remembered, settings changes are applied ----
  const persistStyle = useStylePersistence();
  const setColor = useCallback(
    (c: string) => {
      setColorState(c);
      persistStyle({ annotationColor: c });
    },
    [persistStyle],
  );
  const setStrokeLevel = useCallback(
    (level: number) => {
      const l = clampStrokeLevel(level);
      setStrokeLevelState(l);
      persistStyle({ annotationStroke: l });
    },
    [persistStyle],
  );
  useEffect(() => {
    if (initialColor) setColorState(initialColor);
  }, [initialColor]);
  useEffect(() => {
    if (initialStrokeLevel) setStrokeLevelState(clampStrokeLevel(initialStrokeLevel));
  }, [initialStrokeLevel]);

  // ---- transformer attachment ----
  useEffect(() => {
    const tr = trRef.current;
    const layer = layerRef.current;
    if (!tr || !layer) return;
    const shape = shapes.find((s) => s.id === selectedId);
    const node = shape ? layer.findOne(`#${shape.id}`) : null;
    const transformable = shape && !LINE_TOOLS.has(shape.type) && shape.type !== "pen" && shape.type !== "number";
    if (node && transformable && tool === "select") {
      tr.nodes([node]);
      tr.keepRatio(shape.type === "text");
      tr.enabledAnchors(
        shape.type === "text"
          ? ["top-left", "top-right", "bottom-left", "bottom-right"]
          : ["top-left", "top-center", "top-right", "middle-left", "middle-right", "bottom-left", "bottom-center", "bottom-right"],
      );
    } else {
      tr.nodes([]);
    }
    tr.getLayer()?.batchDraw();
  }, [selectedId, shapes, tool]);

  useEffect(() => {
    if (tool !== "select") setSelectedId(null);
  }, [tool]);

  const reset = useCallback(() => {
    history.reset([]);
    setDraft(null);
    setSelectedId(null);
    setEditingId(null);
    drawingRef.current = false;
  }, [history]);

  const updateShape = useCallback(
    (shape: Shape) => history.set((list) => list.map((s) => (s.id === shape.id ? shape : s))),
    [history],
  );

  const deleteSelected = useCallback(() => {
    if (!selectedId) return;
    history.set((list) => list.filter((s) => s.id !== selectedId));
    setSelectedId(null);
  }, [history, selectedId]);

  /** Pointer position in image pixels, clamped to the surface. */
  const pointer = useCallback((): { x: number; y: number } | null => {
    const stage = stageRef.current;
    const p = stage?.getPointerPosition();
    if (!stage || !p) return null;
    const { width, height, scale } = boundsRef.current;
    return {
      x: Math.max(0, Math.min(width, p.x / scale)),
      y: Math.max(0, Math.min(height, p.y / scale)),
    };
  }, []);

  const onMouseDown = (e: KonvaEventObject<MouseEvent>) => {
    if (editingId || !tool) return;
    const p = pointer();
    if (!p) return;
    if (tool === "select") {
      if (e.target === e.target.getStage()) setSelectedId(null);
      return;
    }
    if (e.evt.button !== 0) return;
    const base = { id: newId(), color, strokeWidth };
    if (tool === "text") {
      // The browser's default mousedown action would move focus to <body>
      // right after we focus the textarea, blurring (and discarding) it.
      e.evt.preventDefault();
      const shape: TextShape = { ...base, type: "text", x: p.x, y: p.y, text: "", fontSize: fontSizeFor(strokeWidth, pixelRatio) };
      history.set((list) => [...list, shape]);
      setEditingId(shape.id);
      return;
    }
    if (tool === "number") {
      const n = shapes.filter((s) => s.type === "number").length + 1;
      history.set((list) => [...list, { ...base, type: "number", x: p.x, y: p.y, n, radius: badgeRadiusFor(strokeWidth, pixelRatio) }]);
      return;
    }
    drawingRef.current = true;
    if (BOX_TOOLS.has(tool)) {
      setDraft({ ...base, type: tool as BoxShape["type"], x: p.x, y: p.y, width: 0, height: 0 });
    } else if (LINE_TOOLS.has(tool)) {
      setDraft({ ...base, type: tool as LineShape["type"], points: [p.x, p.y, p.x, p.y] });
    } else if (tool === "pen") {
      setDraft({ ...base, type: "pen", points: [p.x, p.y] });
    }
  };

  const onMouseMove = () => {
    if (!drawingRef.current || !draft) return;
    const p = pointer();
    if (!p) return;
    setDraft((d) => {
      if (!d) return d;
      if (BOX_TOOLS.has(d.type)) {
        const b = d as BoxShape;
        return { ...b, width: p.x - b.x, height: p.y - b.y };
      }
      if (LINE_TOOLS.has(d.type)) {
        const l = d as LineShape;
        return { ...l, points: [l.points[0], l.points[1], p.x, p.y] };
      }
      if (d.type === "pen") {
        const pen = d as PenShape;
        return { ...pen, points: [...pen.points, p.x, p.y] };
      }
      return d;
    });
  };

  const onMouseUp = () => {
    if (!drawingRef.current) return;
    drawingRef.current = false;
    if (!draft) return;
    let final: Shape | null = draft;
    if (BOX_TOOLS.has(draft.type)) {
      const b = draft as BoxShape;
      const x = Math.min(b.x, b.x + b.width);
      const y = Math.min(b.y, b.y + b.height);
      const width = Math.abs(b.width);
      const height = Math.abs(b.height);
      final = width >= 3 && height >= 3 ? { ...b, x, y, width, height } : null;
    } else if (LINE_TOOLS.has(draft.type)) {
      const [x1, y1, x2, y2] = (draft as LineShape).points;
      final = Math.hypot(x2 - x1, y2 - y1) >= 4 ? draft : null;
    } else if (draft.type === "pen") {
      final = (draft as PenShape).points.length >= 4 ? draft : null;
    }
    if (final) history.set((list) => [...list, final]);
    setDraft(null);
  };

  // ---- text editing ----
  const editingShape = editingId ? (shapes.find((s) => s.id === editingId) as TextShape | undefined) : undefined;

  const commitText = useCallback(
    (text: string) => {
      if (!editingId) return;
      const trimmed = text.replace(/\s+$/g, "");
      history.set((list) =>
        trimmed ? list.map((s) => (s.id === editingId ? { ...(s as TextShape), text: trimmed } : s)) : list.filter((s) => s.id !== editingId),
      );
      setEditingId(null);
    },
    [editingId, history],
  );

  const startEditText = useCallback((id: string) => {
    setSelectedId(null);
    setEditingId(id);
  }, []);

  // ---- export ----
  /** The annotated image at full resolution, as PNG bytes. */
  const renderPng = useCallback(async (): Promise<Uint8Array> => {
    const stage = stageRef.current;
    if (!stage) throw new Error("nothing to export");
    const tr = trRef.current;
    tr?.visible(false);
    try {
      // Read the live scale from the stage: the export must not depend on
      // React state that may be stale in whichever closure called us.
      const s = stage.scaleX() || 1;
      const { width, height } = boundsRef.current;
      const canvas = stage.toCanvas({
        x: 0,
        y: 0,
        width: width * s,
        height: height * s,
        pixelRatio: 1 / s,
      });
      const blob = await new Promise<Blob>((resolve, reject) =>
        canvas.toBlob((b) => (b ? resolve(b) : reject(new Error("toBlob failed"))), "image/png"),
      );
      return new Uint8Array(await blob.arrayBuffer());
    } finally {
      tr?.visible(true);
    }
  }, []);

  // ---- keyboard ----
  /** Handles the editing shortcuts (undo, delete, tool letters). Returns true when consumed. */
  const handleKey = useCallback(
    (e: KeyboardEvent): boolean => {
      if (editingId) return true; // the textarea owns the keyboard
      const mod = isMac ? e.metaKey : e.ctrlKey;
      const key = e.key.toLowerCase();
      if (mod && key === "z") {
        e.preventDefault();
        if (e.shiftKey) history.redo();
        else history.undo();
        return true;
      }
      if (mod && key === "y") {
        e.preventDefault();
        history.redo();
        return true;
      }
      if (e.key === "Escape" && selectedId) {
        e.preventDefault();
        setSelectedId(null);
        return true;
      }
      if ((e.key === "Delete" || e.key === "Backspace") && selectedId) {
        e.preventDefault();
        deleteSelected();
        return true;
      }
      if (!e.metaKey && !e.ctrlKey && !e.altKey) {
        const t = TOOLS.find((x) => x.key.toLowerCase() === key);
        if (t) {
          setTool(t.id);
          return true;
        }
        // 1–5 pick the stroke level.
        if (/^[1-5]$/.test(e.key)) {
          setStrokeLevel(Number(e.key));
          return true;
        }
      }
      return false;
    },
    [editingId, history, selectedId, deleteSelected, setStrokeLevel],
  );

  return {
    history,
    shapes,
    draft,
    tool,
    setTool,
    color,
    setColor,
    strokeLevel,
    setStrokeLevel,
    strokeWidth,
    selectedId,
    setSelectedId,
    editingId,
    editingShape,
    startEditText,
    commitText,
    stageRef,
    layerRef,
    trRef,
    reset,
    updateShape,
    deleteSelected,
    onMouseDown,
    onMouseMove,
    onMouseUp,
    renderPng,
    handleKey,
  };
}

export type Annotator = ReturnType<typeof useAnnotator>;
