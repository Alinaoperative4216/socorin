import { useCallback, useState } from "react";

interface History<T> {
  past: T[];
  present: T;
  future: T[];
}

const LIMIT = 100;

/** Undo/redo stack around a single value. */
export function useHistory<T>(initial: T) {
  const [h, setH] = useState<History<T>>({ past: [], present: initial, future: [] });

  const set = useCallback((next: T | ((prev: T) => T)) => {
    setH((s) => {
      const value = typeof next === "function" ? (next as (prev: T) => T)(s.present) : next;
      if (value === s.present) return s;
      return { past: [...s.past, s.present].slice(-LIMIT), present: value, future: [] };
    });
  }, []);

  const undo = useCallback(() => {
    setH((s) => {
      if (s.past.length === 0) return s;
      const past = s.past.slice(0, -1);
      return { past, present: s.past[s.past.length - 1], future: [s.present, ...s.future] };
    });
  }, []);

  const redo = useCallback(() => {
    setH((s) => {
      if (s.future.length === 0) return s;
      const [next, ...future] = s.future;
      return { past: [...s.past, s.present], present: next, future };
    });
  }, []);

  const reset = useCallback((value: T) => setH({ past: [], present: value, future: [] }), []);

  return {
    value: h.present,
    set,
    undo,
    redo,
    reset,
    canUndo: h.past.length > 0,
    canRedo: h.future.length > 0,
  };
}
