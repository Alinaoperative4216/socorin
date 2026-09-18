import { useEffect, useRef } from "react";
import Konva from "konva";
import type { KonvaEventObject } from "konva/lib/Node";
import { Arrow, Circle, Ellipse, Group, Image as KImage, Line, Rect, Text } from "react-konva";
import type { BoxShape, LineShape, NumberShape, PenShape, Shape, TextShape } from "./types";

interface Props {
  shape: Shape;
  /** The picture being annotated (for the pixelate tool). */
  image: HTMLImageElement | HTMLCanvasElement;
  imageWidth: number;
  imageHeight: number;
  interactive: boolean;
  hidden?: boolean;
  onSelect: (id: string) => void;
  onChange: (shape: Shape) => void;
  onEditText?: (id: string) => void;
}

type NodeEvent = KonvaEventObject<Event>;

function useSelectHandlers(shape: Shape, onSelect: (id: string) => void) {
  const handler = (e: NodeEvent) => {
    e.cancelBubble = true;
    onSelect(shape.id);
  };
  return { onClick: handler, onTap: handler, onMouseDown: handler };
}

function BoxNode({ shape, interactive, onSelect, onChange }: Props & { shape: BoxShape }) {
  const select = useSelectHandlers(shape, onSelect);
  const common = {
    id: shape.id,
    draggable: interactive,
    ...select,
    onDragEnd: (e: NodeEvent) => {
      const n = e.target;
      const dx = shape.type === "ellipse" ? shape.width / 2 : 0;
      const dy = shape.type === "ellipse" ? shape.height / 2 : 0;
      onChange({ ...shape, x: n.x() - dx, y: n.y() - dy });
    },
    onTransformEnd: (e: NodeEvent) => {
      const n = e.target;
      const width = Math.max(2, n.width() * n.scaleX());
      const height = Math.max(2, n.height() * n.scaleY());
      n.scale({ x: 1, y: 1 });
      if (shape.type === "ellipse") {
        onChange({ ...shape, x: n.x() - width / 2, y: n.y() - height / 2, width, height });
      } else {
        onChange({ ...shape, x: n.x(), y: n.y(), width, height });
      }
    },
  };

  if (shape.type === "ellipse") {
    return (
      <Ellipse
        {...common}
        x={shape.x + shape.width / 2}
        y={shape.y + shape.height / 2}
        radiusX={shape.width / 2}
        radiusY={shape.height / 2}
        stroke={shape.color}
        strokeWidth={shape.strokeWidth}
        hitStrokeWidth={Math.max(16, shape.strokeWidth)}
      />
    );
  }
  if (shape.type === "highlight") {
    return (
      <Rect
        {...common}
        x={shape.x}
        y={shape.y}
        width={shape.width}
        height={shape.height}
        fill={shape.color}
        opacity={0.35}
      />
    );
  }
  return (
    <Rect
      {...common}
      x={shape.x}
      y={shape.y}
      width={shape.width}
      height={shape.height}
      stroke={shape.color}
      strokeWidth={shape.strokeWidth}
      cornerRadius={2}
      hitStrokeWidth={Math.max(16, shape.strokeWidth)}
    />
  );
}

function BlurNode({ shape, image, imageWidth, imageHeight, interactive, onSelect, onChange }: Props & { shape: BoxShape }) {
  const ref = useRef<Konva.Image>(null);
  const select = useSelectHandlers(shape, onSelect);
  const crop = {
    x: Math.max(0, Math.min(imageWidth - 1, shape.x)),
    y: Math.max(0, Math.min(imageHeight - 1, shape.y)),
    width: Math.max(1, Math.min(shape.width, imageWidth - shape.x)),
    height: Math.max(1, Math.min(shape.height, imageHeight - shape.y)),
  };
  const pixelSize = Math.max(Math.round(shape.strokeWidth * 3), Math.round(Math.min(shape.width, shape.height) / 12));

  useEffect(() => {
    const node = ref.current;
    if (!node || shape.width < 1 || shape.height < 1) return;
    node.cache({ pixelRatio: 1 });
    node.getLayer()?.batchDraw();
  }, [crop.x, crop.y, crop.width, crop.height, shape.width, shape.height, image, pixelSize]);

  return (
    <KImage
      ref={ref}
      id={shape.id}
      image={image}
      x={shape.x}
      y={shape.y}
      width={shape.width}
      height={shape.height}
      crop={crop}
      filters={[Konva.Filters.Pixelate]}
      pixelSize={pixelSize}
      draggable={interactive}
      {...select}
      onDragEnd={(e) => onChange({ ...shape, x: e.target.x(), y: e.target.y() })}
      onTransformEnd={(e) => {
        const n = e.target;
        const width = Math.max(2, n.width() * n.scaleX());
        const height = Math.max(2, n.height() * n.scaleY());
        n.scale({ x: 1, y: 1 });
        onChange({ ...shape, x: n.x(), y: n.y(), width, height });
      }}
    />
  );
}

function LineNode({ shape, interactive, onSelect, onChange }: Props & { shape: LineShape | PenShape }) {
  const select = useSelectHandlers(shape, onSelect);
  const onDragEnd = (e: NodeEvent) => {
    const n = e.target;
    const dx = n.x();
    const dy = n.y();
    n.position({ x: 0, y: 0 });
    onChange({ ...shape, points: shape.points.map((v, i) => (i % 2 === 0 ? v + dx : v + dy)) });
  };
  const common = {
    id: shape.id,
    points: shape.points,
    stroke: shape.color,
    strokeWidth: shape.strokeWidth,
    lineCap: "round" as const,
    lineJoin: "round" as const,
    hitStrokeWidth: Math.max(18, shape.strokeWidth * 2),
    draggable: interactive,
    ...select,
    onDragEnd,
  };
  if (shape.type === "arrow") {
    return (
      <Arrow
        {...common}
        fill={shape.color}
        pointerLength={shape.strokeWidth * 3.5 + 4}
        pointerWidth={shape.strokeWidth * 3 + 4}
      />
    );
  }
  return <Line {...common} tension={shape.type === "pen" ? 0.4 : 0} />;
}

function TextNode({ shape, interactive, hidden, onSelect, onChange, onEditText }: Props & { shape: TextShape }) {
  const select = useSelectHandlers(shape, onSelect);
  return (
    <Text
      id={shape.id}
      x={shape.x}
      y={shape.y}
      text={shape.text}
      fontSize={shape.fontSize}
      fontStyle="bold"
      fontFamily="-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif"
      lineHeight={1}
      fill={shape.color}
      shadowColor="rgba(0,0,0,0.6)"
      shadowBlur={3}
      shadowOffset={{ x: 1, y: 1 }}
      visible={!hidden}
      draggable={interactive}
      {...select}
      onDblClick={() => onEditText?.(shape.id)}
      onDblTap={() => onEditText?.(shape.id)}
      onDragEnd={(e) => onChange({ ...shape, x: e.target.x(), y: e.target.y() })}
      onTransformEnd={(e) => {
        const n = e.target;
        const scale = Math.max(n.scaleX(), n.scaleY());
        n.scale({ x: 1, y: 1 });
        onChange({ ...shape, x: n.x(), y: n.y(), fontSize: Math.max(8, Math.round(shape.fontSize * scale)) });
      }}
    />
  );
}

function NumberNode({ shape, interactive, onSelect, onChange }: Props & { shape: NumberShape }) {
  const select = useSelectHandlers(shape, onSelect);
  const r = shape.radius;
  return (
    <Group
      id={shape.id}
      x={shape.x}
      y={shape.y}
      draggable={interactive}
      {...select}
      onDragEnd={(e) => onChange({ ...shape, x: e.target.x(), y: e.target.y() })}
    >
      <Circle radius={r} fill={shape.color} stroke="#ffffff" strokeWidth={Math.max(1.5, r / 8)} shadowColor="rgba(0,0,0,0.5)" shadowBlur={r / 4} />
      <Text
        text={String(shape.n)}
        fontSize={r * 1.2}
        fontStyle="bold"
        fill="#ffffff"
        width={r * 2}
        height={r * 2}
        offsetX={r}
        offsetY={r}
        align="center"
        verticalAlign="middle"
      />
    </Group>
  );
}

export function ShapeNode(props: Props) {
  const { shape } = props;
  switch (shape.type) {
    case "rect":
    case "ellipse":
    case "highlight":
      return <BoxNode {...props} shape={shape} />;
    case "blur":
      return <BlurNode {...props} shape={shape} />;
    case "arrow":
    case "line":
    case "pen":
      return <LineNode {...props} shape={shape} />;
    case "text":
      return <TextNode {...props} shape={shape} />;
    case "number":
      return <NumberNode {...props} shape={shape} />;
    default:
      return null;
  }
}
