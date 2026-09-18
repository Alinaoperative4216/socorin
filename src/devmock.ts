/**
 * Browser-only stand-in for the Tauri runtime so the UI can be opened in a
 * plain browser for visual QA: `http://localhost:1420/?mock=overlay-1`,
 * `?mock=editor` or `?mock=main`. Only ever loaded in dev builds.
 */

type Handler = (args: unknown, options?: unknown) => Promise<unknown>;

let settings = {
  hotkey: "CmdOrCtrl+Shift+A",
  fullscreenHotkey: "",
  recordHotkey: "",
  saveDir: "/Users/mock/Pictures/Screenshots",
  afterCapture: "editor",
  copyOnSave: true,
  autostart: true,
  cliTriggers: false,
  filePrefix: "Socorin",
  ffmpegPath: "",
  mic: true,
  micDevice: "",
  micDeviceName: "",
  welcomeShown: true,
  annotationColor: "#ff3b30",
  annotationStroke: 3,
  checkUpdates: true,
  autoUpdate: true,
  updateCheckedAt: Math.floor(Date.now() / 1000) - 3600,
  updateAvailable: "1.0.9",
  lastVersion: "1.0.2",
  uploadServer: "https://socorin.com",
  installId: "mockInstallId0123456789",
};

// `?mock=share&state=failed` shows the popover's failure look.
let mockLinks = [
  {
    id: "med-0123456789abcdefgh",
    shareUrl: "https://socorin.com/s/med-0123456789abcdefgh",
    expiresAt: new Date(Date.now() + 60 * 86_400_000).toISOString(),
    kind: "image",
    mime: "image/png",
    size: 123_456,
    createdAt: Math.floor(Date.now() / 1000) - 120,
  },
];
function mockShareNotice() {
  const state = new URLSearchParams(location.search).get("state");
  return state === "failed"
    ? { kind: "failed", error: { code: "file_too_large", message: "This capture is 7.3 MB; the share limit is 5 MB.", maxBytes: 5_242_880 } }
    : { kind: "shared", link: mockLinks[0], retentionDays: 60 };
}

// `?mock=update&state=installing|failed|updated` shows the popover's other looks.
function mockUpdateStatus() {
  const state = new URLSearchParams(location.search).get("state");
  return {
    currentVersion: "1.0.2",
    available: settings.updateAvailable,
    checkedAt: settings.updateCheckedAt,
    phase:
      state === "installing"
        ? { phase: "installing", received: 3_000_000, total: 5_400_000 }
        : state === "failed"
          ? { phase: "failed", error: "There is no automatic update for this platform yet. Get the new version from socorin.com." }
          : { phase: "idle" },
    justUpdated: state === "updated",
  };
}

function paintMockScreen(width: number, height: number): HTMLCanvasElement {
  const c = document.createElement("canvas");
  c.width = width;
  c.height = height;
  const ctx = c.getContext("2d")!;
  const g = ctx.createLinearGradient(0, 0, width, height);
  g.addColorStop(0, "#1e3a8a");
  g.addColorStop(1, "#7e22ce");
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, width, height);
  const step = Math.round(Math.min(width, height) / 12);
  ctx.strokeStyle = "rgba(255,255,255,0.22)";
  ctx.lineWidth = 1;
  for (let x = 0; x < width; x += step) {
    ctx.beginPath();
    ctx.moveTo(x + 0.5, 0);
    ctx.lineTo(x + 0.5, height);
    ctx.stroke();
  }
  for (let y = 0; y < height; y += step) {
    ctx.beginPath();
    ctx.moveTo(0, y + 0.5);
    ctx.lineTo(width, y + 0.5);
    ctx.stroke();
  }
  ctx.fillStyle = "rgba(255,255,255,0.9)";
  ctx.font = `bold ${Math.round(step * 0.6)}px sans-serif`;
  ctx.fillText("Mock screen " + width + "×" + height, step, step * 1.5);
  ctx.fillStyle = "#facc15";
  ctx.fillRect(width * 0.6, height * 0.6, width * 0.25, height * 0.2);
  ctx.fillStyle = "#111";
  ctx.font = `${Math.round(step * 0.4)}px sans-serif`;
  ctx.fillText("Some text to pixelate", width * 0.62, height * 0.7);
  return c;
}

const dpr = window.devicePixelRatio || 1;
// The pane may not be laid out yet when the first command arrives.
const viewport = () => ({
  w: window.innerWidth || document.documentElement.clientWidth || 1280,
  h: window.innerHeight || document.documentElement.clientHeight || 800,
});
const monitor = () => {
  const { w, h } = viewport();
  return {
    id: 1,
    name: "Mock display",
    x: 0,
    y: 0,
    width: w,
    height: h,
    scale: dpr,
    primary: true,
    imageWidth: Math.round(w * dpr),
    imageHeight: Math.round(h * dpr),
  };
};

const handlers: Record<string, Handler> = {
  overlay_info: async () => ({ ...monitor(), session: 1, preselectFull: !!new URLSearchParams(location.search).get("full"), mode: (new URLSearchParams(location.search).get("record") ? "record" : "screenshot") }),
  begin_annotation: async (args) => console.log("[mock] begin_annotation", JSON.stringify(args)),
  cancel_capture: async () => {
    console.log("[mock] cancel_capture → reload");
    window.setTimeout(() => location.reload(), 300);
  },
  save_png_as: async (args) => {
    console.log("[mock] save_png_as", (args as ArrayBuffer).byteLength, "bytes");
    return "/Users/mock/Pictures/Screenshots/save-as.png";
  },
  edit_png: async (args) => console.log("[mock] edit_png", (args as ArrayBuffer).byteLength, "bytes"),
  overlay_ready: async (args) => console.log("[mock] overlay_ready", JSON.stringify(args)),
  overlay_pixels: async () => {
    const m = monitor();
    const c = paintMockScreen(m.imageWidth, m.imageHeight);
    return c.getContext("2d")!.getImageData(0, 0, m.imageWidth, m.imageHeight).data.buffer;
  },
  pending_image: async () => {
    const c = paintMockScreen(1400, 900);
    const blob = await new Promise<Blob>((res) => c.toBlob((b) => res(b!), "image/png"));
    return blob.arrayBuffer();
  },
  debug_options: async () => ({ enabled: false, dumpDir: null, autoSelect: null, autoAction: null }),
  debug_log: async (args) => console.log("[mock] debug_log", (args as { message: string }).message),
  // `?mock=main&noperm=1&issue=disk-image&mic=denied` exercises the warning cards.
  platform_info: async () => {
    const q = new URLSearchParams(location.search);
    return {
      os: "macos",
      screenPermission: !q.get("noperm"),
      micPermission: q.get("mic") ?? "granted",
      wayland: false,
      installIssue: q.get("issue") ?? null,
    };
  },
  audio_inputs: async () => [
    { id: "BuiltInMicrophoneDevice", name: "MacBook Pro Microphone", default: true },
    { id: "AppleUSBAudioEngine:Jabra:2", name: "Jabra Speak 710", default: false },
  ],
  open_mic_permission_settings: async () => console.log("[mock] open_mic_permission_settings"),
  get_settings: async () => settings,
  update_settings: async (args) => {
    settings = { ...settings, ...(args as { patch: Partial<typeof settings> }).patch };
    return settings;
  },
  welcome_ready: async () => undefined,
  dismiss_welcome: async () => undefined,
  update_status: async () => mockUpdateStatus(),
  check_for_updates: async () => {
    await new Promise((r) => setTimeout(r, 800));
    settings.updateCheckedAt = Math.floor(Date.now() / 1000);
    return mockUpdateStatus();
  },
  install_update: async () => console.log("[mock] install_update"),
  dismiss_update: async () => console.log("[mock] dismiss_update"),
  update_ready: async () => undefined,
  start_record_region: async () => console.log("[mock] start_record_region"),
  start_record_fullscreen: async () => console.log("[mock] start_record_fullscreen"),
  start_recording: async (args) => console.log("[mock] start_recording", args),
  stop_recording: async () => console.log("[mock] stop_recording"),
  stop_recording_copy: async () => console.log("[mock] stop_recording_copy"),
  cancel_recording: async () => console.log("[mock] cancel_recording"),
  stop_recording_upload: async (args) => console.log("[mock] stop_recording_upload", JSON.stringify(args)),
  recording_status: async () => ({
    recording: true,
    startedMs: Date.now() - 65_000,
    path: "/Users/mock/Pictures/Screenshots/Screenshot.mov",
    audio: settings.mic ? "MacBook Pro Microphone" : null,
    audioIssue: null,
  }),
  upload_png: async (args) => {
    console.log("[mock] upload_png", (args as ArrayBuffer).byteLength, "bytes");
    await new Promise((r) => setTimeout(r, 1200));
    // No delete token: it never leaves the Rust side (deleting goes by id).
    return mockLinks[0];
  },
  share_history: async () => mockLinks,
  delete_share: async (args) => {
    mockLinks = mockLinks.filter((l) => l.id !== (args as { id: string }).id);
  },
  copy_share_link: async (args) => console.log("[mock] copy_share_link", (args as { id: string }).id),
  reset_install_id: async () => {
    settings = { ...settings, installId: "" };
    return settings;
  },
  share_notice: async () => mockShareNotice(),
  share_ready: async () => undefined,
  dismiss_share: async () => console.log("[mock] dismiss_share"),
  share_engaged: async () => console.log("[mock] share_engaged"),
  default_save_dir: async () => "/Users/mock/Pictures/Screenshots",
  save_png: async (args) => {
    const bytes = args instanceof Uint8Array ? args.byteLength : (args as ArrayBuffer).byteLength;
    console.log("[mock] save_png", bytes, "bytes");
    return "/Users/mock/Pictures/Screenshots/mock.png";
  },
  copy_png: async (args) => {
    console.log("[mock] copy_png", (args as ArrayBuffer).byteLength, "bytes");
  },
  finish_region: async (args) => console.log("[mock] finish_region", JSON.stringify(args)),
  "plugin:dialog|open": async () => "/Users/mock/Pictures/Other",
  "plugin:event|listen": async () => 1,
};

let callbackId = 0;

export function installMock(label: string) {
  (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
    metadata: {
      currentWindow: { label },
      currentWebview: { label, windowLabel: label },
    },
    invoke: async (cmd: string, args?: unknown, options?: unknown) => {
      const handler = handlers[cmd];
      if (handler) return handler(args, options);
      console.log("[mock]", cmd, args ?? "");
      return null;
    },
    transformCallback: (cb: (...a: unknown[]) => void) => {
      callbackId += 1;
      (window as unknown as Record<string, unknown>)[`_${callbackId}`] = cb;
      return callbackId;
    },
    unregisterCallback: (id: number) => {
      delete (window as unknown as Record<string, unknown>)[`_${id}`];
    },
    convertFileSrc: (path: string) => path,
  };
  console.log(`[mock] Tauri runtime mocked as window "${label}"`);
}
