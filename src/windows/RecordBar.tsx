import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { Bar } from "../lib/ipc";
import type { Crop } from "./InPlaceEditor";
import { RecordControls } from "./RecordControls";

interface Props {
  crop: Crop;
  /** CSS pixels per bitmap pixel. */
  scale: number;
  /** The selection is being moved / resized: keep out of the way. */
  hidden: boolean;
  /** Start; `bar` is where this bar is, so the recording bar can take its place. */
  onStart: (bar: Bar) => void;
  onCancel: () => void;
}

const GAP = 8;

/**
 * Record mode of the overlay: the selected area stays adjustable (handles,
 * drag to move) and this bar floats next to it with Record / Cancel.
 * Enter starts, Esc cancels.
 */
export function RecordBar({ crop, scale, hidden, onStart, onCancel }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setSize({ w: el.offsetWidth, h: el.offsetHeight });
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    measure();
    return () => ro.disconnect();
  }, []);

  const box = { left: crop.x * scale, top: crop.y * scale, width: crop.width * scale, height: crop.height * scale };
  const pos = useMemo(() => {
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    let top = box.top + box.height + GAP;
    if (top + size.h > vh - GAP) {
      top = box.top - GAP - size.h;
      if (top < GAP) top = Math.max(GAP, box.top + box.height - GAP - size.h);
    }
    const left = Math.max(GAP, Math.min(box.left + box.width - size.w, vw - size.w - GAP));
    return { left, top };
  }, [box.left, box.top, box.width, box.height, size]);

  const start = () => {
    const r = ref.current?.getBoundingClientRect();
    onStart(r ? { x: r.left, y: r.top, width: r.width, height: r.height } : { x: pos.left, y: pos.top, width: size.w, height: size.h });
  };
  const startRef = useRef(start);
  startRef.current = start;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Enter") {
        e.preventDefault();
        startRef.current();
      } else if (e.key === "Escape") {
        e.preventDefault();
        onCancel();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel]);

  return (
    <div className="inplace" style={{ visibility: hidden ? "hidden" : "visible" }}>
      <div ref={ref} className="floating-toolbar" style={pos}>
        <RecordControls phase="ready" elapsedMs={0} onRecord={start} onCancel={onCancel} />
      </div>
    </div>
  );
}
