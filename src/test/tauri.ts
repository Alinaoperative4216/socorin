/**
 * In-page stand-in for the Tauri runtime, the same hook the real
 * `@tauri-apps/api` uses (`window.__TAURI_INTERNALS__`), so the UI code and
 * the API package run unmodified. Commands are answered by `handlers`, events
 * are delivered with `emit`.
 */
import { vi, type Mock } from "vitest";
import type { Settings, UpdateStatus } from "../lib/ipc";

export type Handler = (args: unknown, options?: unknown) => unknown;

interface Listener {
  id: number;
  event: string;
  target: { kind: string; label?: string };
}

export interface TauriMock {
  /** Every `invoke(cmd, args, options)`; assert on it or read `calls`. */
  invoke: Mock<(cmd: string, args?: unknown, options?: unknown) => Promise<unknown>>;
  handlers: Record<string, Handler>;
  /** The `args` of every invocation of `cmd`. */
  calls: (cmd: string) => unknown[];
  /** Deliver an event to the listeners registered for it. */
  emit: (event: string, payload?: unknown) => void;
  listeners: () => Listener[];
  label: string;
}

/**
 * Install the fake runtime as window `label`. Commands without a handler
 * resolve to `undefined`; a handler may throw / return a rejected promise to
 * simulate a command error.
 */
export function installTauri(label = "main", handlers: Record<string, Handler> = {}): TauriMock {
  const callbacks = new Map<number, (payload: unknown) => void>();
  const listeners: Listener[] = [];
  let nextId = 0;

  const invoke = vi.fn(async (cmd: string, args?: unknown, options?: unknown): Promise<unknown> => {
    if (cmd === "plugin:event|listen") {
      const a = args as { event: string; target: Listener["target"]; handler: number };
      listeners.push({ id: a.handler, event: a.event, target: a.target });
      return a.handler;
    }
    if (cmd === "plugin:event|unlisten") {
      const a = args as { event: string; eventId: number };
      const i = listeners.findIndex((l) => l.id === a.eventId);
      if (i >= 0) listeners.splice(i, 1);
      return undefined;
    }
    const handler = handlers[cmd];
    return handler ? handler(args, options) : undefined;
  });

  const internals = {
    metadata: {
      currentWindow: { label },
      currentWebview: { label, windowLabel: label },
      windows: [{ label }],
      webviews: [{ label, windowLabel: label }],
    },
    invoke,
    transformCallback: (cb: (payload: unknown) => void) => {
      nextId += 1;
      callbacks.set(nextId, cb);
      return nextId;
    },
    unregisterCallback: (id: number) => {
      callbacks.delete(id);
    },
    convertFileSrc: (path: string) => path,
  };
  (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = internals;
  (window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__: unknown }).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener: (_event: string, id: number) => callbacks.delete(id),
  };

  return {
    invoke,
    handlers,
    label,
    calls: (cmd) => invoke.mock.calls.filter((c) => c[0] === cmd).map((c) => c[1]),
    emit: (event, payload) => {
      for (const l of [...listeners]) {
        if (l.event !== event) continue;
        callbacks.get(l.id)?.({ event, id: l.id, payload });
      }
    },
    listeners: () => [...listeners],
  };
}

/** Let pending promises / microtasks settle. */
export const flush = () => new Promise<void>((r) => setTimeout(r, 0));

export const SETTINGS: Settings = {
  hotkey: "CmdOrCtrl+Shift+A",
  fullscreenHotkey: "",
  recordHotkey: "",
  saveDir: "/tmp/shots",
  afterCapture: "editor",
  copyOnSave: true,
  autostart: true,
  cliTriggers: false,
  filePrefix: "Socorin",
  ffmpegPath: "",
  welcomeShown: true,
  annotationColor: "#ff3b30",
  annotationStroke: 3,
  checkUpdates: true,
  autoUpdate: false,
  updateCheckedAt: 0,
  updateAvailable: "",
  lastVersion: "1.0.2",
};

export const UPDATE_STATUS: UpdateStatus = {
  currentVersion: "1.0.2",
  available: "",
  checkedAt: 0,
  phase: { phase: "idle" },
  justUpdated: false,
};
