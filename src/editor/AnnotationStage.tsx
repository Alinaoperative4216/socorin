import { useEffect, useRef } from "react";
import { Image as KImage, Layer, Stage, Transformer } from "react-konva";
import { ShapeNode } from "./Shapes";
import type { Annotator } from "./useAnnotator";

export type Bitmap = HTMLImageElement | HTMLCanvasElement;

interface Props {
  a: Annotator;
  /** The picture being annotated, exactly `width` × `height` pixels. */
  image: Bitmap;
  width: number;
  height: number;
  /** Stage pixels per image pixel. */
  scale: number;
}

/**
 * The Konva stage with the picture, the annotations, the transformer and the
 * text-editing textarea. All state lives in the `Annotator` so the editor
 * window and the in-place editor can share it.
 */
export function AnnotationStage({ a, image, width, height, scale }: Props) {
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const { editingShape } = a;

  useEffect(() => {
    if (!editingShape) return;
    // Focus after the current mouse event has fully settled.
    const timer = window.setTimeout(() => {
      const ta = textareaRef.current;
      if (ta) {
        ta.focus();
        ta.select();
      }
    }, 0);
    return () => window.clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editingShape?.id]);

  const interactive = a.tool === "select" && !a.editingId;
  const cursorClass = a.tool === "select" ? "tool-select" : a.tool === "text" ? "tool-text" : a.tool ? "tool-draw" : "";

  return (
    <div className={`canvas-holder ${cursorClass}`}>
      <Stage
        ref={a.stageRef}
        width={width * scale}
        height={height * scale}
        scaleX={scale}
        scaleY={scale}
        onMouseDown={a.onMouseDown}
        onMouseMove={a.onMouseMove}
        onMouseUp={a.onMouseUp}
        onMouseLeave={a.onMouseUp}
      >
        <Layer listening={false}>
          <KImage image={image} width={width} height={height} />
        </Layer>
        <Layer ref={a.layerRef} listening={interactive}>
          {a.shapes.map((s) => (
            <ShapeNode
              key={s.id}
              shape={s}
              image={image}
              imageWidth={width}
              imageHeight={height}
              interactive={interactive}
              hidden={s.id === a.editingId}
              onSelect={a.setSelectedId}
              onChange={a.updateShape}
              onEditText={a.startEditText}
            />
          ))}
          <Transformer
            ref={a.trRef}
            rotateEnabled={false}
            flipEnabled={false}
            ignoreStroke
            anchorSize={8}
            anchorStroke="#0a84ff"
            anchorFill="#ffffff"
            borderStroke="#0a84ff"
            boundBoxFunc={(oldBox, newBox) => (newBox.width < 4 || newBox.height < 4 ? oldBox : newBox)}
          />
        </Layer>
        <Layer listening={false}>
          {a.draft && (
            <ShapeNode
              shape={a.draft}
              image={image}
              imageWidth={width}
              imageHeight={height}
              interactive={false}
              onSelect={() => {}}
              onChange={() => {}}
            />
          )}
        </Layer>
      </Stage>

      {editingShape && (
        <textarea
          ref={textareaRef}
          className="text-editor"
          defaultValue={editingShape.text}
          style={{
            left: editingShape.x * scale,
            top: editingShape.y * scale,
            fontSize: editingShape.fontSize * scale,
            color: editingShape.color,
            minHeight: editingShape.fontSize * scale * 1.2,
          }}
          rows={1}
          onInput={(e) => {
            const ta = e.currentTarget;
            ta.style.height = "auto";
            ta.style.height = `${ta.scrollHeight}px`;
            ta.style.width = `${Math.max(40, ta.scrollWidth + 8)}px`;
          }}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              a.commitText(e.currentTarget.value);
            } else if (e.key === "Escape") {
              e.preventDefault();
              a.commitText(e.currentTarget.value);
            }
          }}
          onBlur={(e) => a.commitText(e.currentTarget.value)}
        />
      )}
    </div>
  );
}
