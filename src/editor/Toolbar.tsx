import {
  AppWindow,
  Circle,
  Copy,
  EyeOff,
  FolderDown,
  Hash,
  Highlighter,
  Maximize2,
  Minus,
  MousePointer2,
  MoveUpRight,
  Pencil,
  Redo2,
  Save,
  Square,
  Trash2,
  Type,
  Undo2,
  X,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import type { ComponentType, MouseEvent as ReactMouseEvent } from "react";
import { COLORS, STROKE_LEVELS, type ToolId } from "./types";

export const TOOLS: { id: ToolId; label: string; key: string; Icon: ComponentType<{ size?: number }> }[] = [
  { id: "select", label: "Select / move", key: "V", Icon: MousePointer2 },
  { id: "rect", label: "Rectangle", key: "R", Icon: Square },
  { id: "ellipse", label: "Ellipse", key: "E", Icon: Circle },
  { id: "arrow", label: "Arrow", key: "A", Icon: MoveUpRight },
  { id: "line", label: "Line", key: "L", Icon: Minus },
  { id: "pen", label: "Pen", key: "P", Icon: Pencil },
  { id: "text", label: "Text", key: "T", Icon: Type },
  { id: "highlight", label: "Highlight", key: "H", Icon: Highlighter },
  { id: "number", label: "Numbered marker", key: "N", Icon: Hash },
  { id: "blur", label: "Pixelate", key: "B", Icon: EyeOff },
];

interface Props {
  /** `null` = no drawing tool active (only the in-place editor allows that). */
  tool: ToolId | null;
  color: string;
  /** Stroke level 1..5 (see STROKE_LEVELS). */
  strokeLevel: number;
  canUndo: boolean;
  canRedo: boolean;
  hasSelection: boolean;
  busy: boolean;
  onTool: (t: ToolId | null) => void;
  onColor: (c: string) => void;
  onStroke: (level: number) => void;
  onUndo: () => void;
  onRedo: () => void;
  onDelete: () => void;
  onCopy: () => void;
  onSave: () => void;
  onSaveAs: () => void;
  onClose: () => void;
  /** Floating variant (in-place editor): no zoom controls, tools toggle off. */
  floating?: boolean;
  zoom?: number;
  onZoom?: (delta: number) => void;
  onFit?: () => void;
  /** "Open in editor window", floating variant only. */
  onEdit?: () => void;
}

export function Toolbar(p: Props) {
  const tools = TOOLS.map(({ id, label, key, Icon }) => (
    <button
      key={id}
      type="button"
      className={`tool-btn ${p.tool === id ? "active" : ""}`}
      title={`${label} (${key})`}
      onClick={() => p.onTool(p.floating && p.tool === id ? null : id)}
    >
      <Icon size={17} />
    </button>
  ));

  const style = (
    <>
      <div className="swatches">
        {COLORS.map((c) => (
          <button
            key={c}
            type="button"
            className={`swatch ${p.color === c ? "active" : ""}`}
            style={{ background: c }}
            title={c}
            onClick={() => p.onColor(c)}
          />
        ))}
        <span className={`swatch custom ${COLORS.includes(p.color) ? "" : "active"}`} title="Custom colour">
          <input type="color" value={p.color} onChange={(e) => p.onColor(e.target.value)} />
        </span>
      </div>

      <span className="sep" />

      <div className="sizes">
        {STROKE_LEVELS.map((px, i) => {
          const level = i + 1;
          return (
            <button
              key={level}
              type="button"
              className={`size-btn ${p.strokeLevel === level ? "active" : ""}`}
              title={`Stroke ${level} (${px}px) — key ${level}`}
              onClick={() => p.onStroke(level)}
            >
              <i style={{ width: 6 + level * 2, height: 6 + level * 2 }} />
            </button>
          );
        })}
      </div>
    </>
  );

  const history = (
    <>
      <button type="button" className="tool-btn" title="Undo (Ctrl/⌘+Z)" disabled={!p.canUndo} onClick={p.onUndo}>
        <Undo2 size={17} />
      </button>
      <button type="button" className="tool-btn" title="Redo (Ctrl/⌘+Shift+Z)" disabled={!p.canRedo} onClick={p.onRedo}>
        <Redo2 size={17} />
      </button>
      <button type="button" className="tool-btn" title="Delete selected (Del)" disabled={!p.hasSelection} onClick={p.onDelete}>
        <Trash2 size={17} />
      </button>
    </>
  );

  const zoom = (
    <>
      <button type="button" className="tool-btn" title="Zoom out" onClick={() => p.onZoom?.(-1)}>
        <ZoomOut size={17} />
      </button>
      <span className="zoom-label">{Math.round((p.zoom ?? 1) * 100)}%</span>
      <button type="button" className="tool-btn" title="Zoom in" onClick={() => p.onZoom?.(1)}>
        <ZoomIn size={17} />
      </button>
      <button type="button" className="tool-btn" title="Fit to window" onClick={p.onFit}>
        <Maximize2 size={17} />
      </button>
    </>
  );

  const actions = (
    <>
      {p.onEdit && (
        <button type="button" className="tool-btn" title="Open in the editor window (Ctrl/⌘+E)" disabled={p.busy} onClick={p.onEdit}>
          <AppWindow size={17} />
        </button>
      )}
      <button type="button" className="tool-btn wide" title="Save to screenshots folder (Ctrl/⌘+S)" disabled={p.busy} onClick={p.onSave}>
        <Save size={16} /> Save
      </button>
      <button type="button" className="tool-btn wide" title="Save as… (Ctrl/⌘+Shift+S)" disabled={p.busy} onClick={p.onSaveAs}>
        <FolderDown size={16} /> Save as
      </button>
      <button
        type="button"
        className="tool-btn wide primary"
        title={p.floating ? "Copy to clipboard and finish (Enter, Ctrl/⌘+C)" : "Copy to clipboard (Ctrl/⌘+C)"}
        disabled={p.busy}
        onClick={p.onCopy}
      >
        <Copy size={16} /> Copy
      </button>
      <button type="button" className="tool-btn wide" title="Close (Esc)" onClick={p.onClose}>
        <X size={16} /> Close
      </button>
    </>
  );

  // Buttons must not take keyboard focus: Enter / Space are editor shortcuts.
  const stopFocus = (e: ReactMouseEvent) => e.preventDefault();

  if (p.floating) {
    // Compact, Lark-like: colours and stroke sizes only appear while a
    // drawing tool is active.
    const showStyle = !!p.tool && p.tool !== "select";
    return (
      <div className="toolbar floating" onMouseDown={stopFocus}>
        <div className="toolbar-row">
          {tools}
          <span className="sep" />
          {history}
          <span className="spacer" />
          {actions}
        </div>
        {showStyle && <div className="toolbar-row">{style}</div>}
      </div>
    );
  }

  return (
    <div className="toolbar" onMouseDown={stopFocus}>
      {tools}
      <span className="sep" />
      {style}
      <span className="sep" />
      {history}
      <span className="sep" />
      {zoom}
      <span className="spacer" />
      {actions}
    </div>
  );
}
