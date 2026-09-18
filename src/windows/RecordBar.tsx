import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ipc, type AudioInput, type Bar, type Settings } from "../lib/ipc";
import { micState } from "../lib/mic";
import type { Crop } from "./InPlaceEditor";
import { RecordControls } from "./RecordControls";

interface Props {
  crop: Crop;
  /** CSS pixels per bitmap pixel. */
  scale: number;
  /** The selection is being moved / resized: keep out of the way. */
  hidden: boolean;
  /** The settings (for the microphone); null when they could not be loaded. */
  settings: Settings | null;
  /** The mic button: switch the sound off / on for this and later takes. */
  onToggleMic: () => void;
  /** Start; `bar` is where this bar is, so the recording bar can take its place. */
  onStart: (bar: Bar) => void;
  onCancel: () => void;
}

const GAP = 8;

/**
 * Record mode of the overlay: the selected area stays adjustable (handles,
 * drag to move) and this bar floats next to it with the microphone
 * button, Record and Cancel. Enter starts, Esc cancels.
 */
export function RecordBar({ crop, scale, hidden, settings, onToggleMic, onStart, onCancel }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });
  // The microphones, for the button's tooltip (which one the recording
  // takes, whether the chosen one is connected). Listed once the bar is
  // up, so the overlay itself stays as quick as before.
  const [inputs, setInputs] = useState<AudioInput[] | null>(null);

  useEffect(() => {
    let live = true;
    ipc.audioInputs()
      .then((list) => {
        if (live) setInputs(Array.isArray(list) ? list : []);
      })
      .catch(() => {
        if (live) setInputs([]);
      });
    return () => {
      live = false;
    };
  }, []);

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

  const mic = useMemo(() => micState(settings, inputs), [settings, inputs]);

  return (
    <div className="inplace" style={{ visibility: hidden ? "hidden" : "visible" }}>
      <div ref={ref} className="floating-toolbar" style={pos}>
        <RecordControls phase="ready" elapsedMs={0} mic={mic} onToggleMic={onToggleMic} onRecord={start} onCancel={onCancel} />
      </div>
    </div>
  );
}
