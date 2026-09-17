import { invoke } from "@tauri-apps/api/core";

export interface MonitorInfo {
  id: number;
  name: string;
  x: number;
  y: number;
  width: number;
  height: number;
  scale: number;
  primary: boolean;
  imageWidth: number;
  imageHeight: number;
  /** Capture session counter; overlays ignore repeated signals for the same session. */
  session: number;
  /** Full-screen capture in annotate mode: select the whole monitor at once. */
  preselectFull: boolean;
  /** What the session is for: a screenshot, or picking an area to record. */
  mode: CaptureMode;
}

export type CaptureMode = "screenshot" | "record";

export interface RecordingStatus {
  recording: boolean;
  /** Unix time in ms when the recording started (0 when idle). */
  startedMs: number;
  path: string;
}

/** Payload of `recording:stopped`. */
export interface RecordingStopped {
  /** The file went to the clipboard (Stop & copy). */
  copied: boolean;
  /** The share link, after Stop & upload. */
  link?: string | null;
}

/**
 * Where the overlay's Record bar is, in CSS pixels of the overlay window
 * (= logical pixels from the monitor's top-left corner). The recording bar
 * window is placed there.
 */
export interface Bar {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * Transparent room the recording bar's window keeps around the bar for its
 * shadow; the bar is drawn this far from the window's edges. Must match
 * `RECORDER_MARGIN` in `src-tauri/src/windows.rs`.
 */
export const RECORDER_MARGIN = 20;

/** A region of a monitor bitmap, in physical pixels. */
export interface Region {
  monitorId: number;
  x: number;
  y: number;
  width: number;
  height: number;
}

export type AfterCapture = "editor" | "clipboard" | "save" | "upload";

export interface Settings {
  hotkey: string;
  fullscreenHotkey: string;
  /** Starts a region recording, or stops the running one. Empty = disabled. */
  recordHotkey: string;
  saveDir: string;
  afterCapture: AfterCapture;
  copyOnSave: boolean;
  autostart: boolean;
  /** Honour --capture / --capture-full / --record / --record-full on the command line. */
  cliTriggers: boolean;
  filePrefix: string;
  /**
   * Linux: the ffmpeg binary for recording, empty = usual locations, then PATH.
   * Windows: empty = the built-in recorder, a path = record with that ffmpeg.
   */
  ffmpegPath: string;
  /** The first-launch welcome dialog has been dismissed. */
  welcomeShown: boolean;
  /** Last annotation colour (#rrggbb) and stroke level (1–5), kept across captures. */
  annotationColor: string;
  annotationStroke: number;
  /** Look for a newer version on socorin.com once a day (default on). */
  checkUpdates: boolean;
  /** Install a newer version as soon as the daily check finds one (default off). */
  autoUpdate: boolean;
  /** Unix seconds of the last successful update check, 0 = never. */
  updateCheckedAt: number;
  /** Newer version the last check found, "" = none. */
  updateAvailable: string;
  /** Version that ran last time (an update was installed when it differs). */
  lastVersion: string;
  /** Where "Upload & copy link" sends captures: an http(s) origin, socorin.com by default. */
  uploadServer: string;
  /** The random id the share server gave this install ("" until the first upload). */
  installId: string;
}

export type ShareKind = "image" | "video";

/** The answer to a finished upload (`upload_png`). */
export interface ShareResult {
  id: string;
  shareUrl: string;
  deleteToken: string;
  /** ISO 8601, when the server removes the file. */
  expiresAt: string;
  kind: ShareKind;
  mime: string;
  size: number;
}

/** A remembered link (`share_history`), without its delete token. */
export interface SharedLink {
  id: string;
  shareUrl: string;
  expiresAt: string;
  kind: ShareKind;
  mime: string;
  size: number;
  /** Unix seconds of the upload. */
  createdAt: number;
}

/**
 * Why an upload did not happen: the server's error code (`file_too_large`,
 * `rate_limited`, `storage_full`, `unsupported_type`, …) or the app's own
 * (`network`, `server`, `invalid_response`), with the sentence to show.
 */
export interface ShareError {
  code: string;
  message: string;
  maxBytes?: number;
  retryAfterSeconds?: number;
}

/** What the "Link copied" popover shows (`share_notice`, event `share:notice`). */
export type ShareNotice =
  | { kind: "shared"; link: SharedLink; retentionDays: number }
  | { kind: "failed"; error: ShareError };

/** The sentence for a rejected share command (a `ShareError`, or any other error). */
export function shareErrorMessage(e: unknown): string {
  if (e && typeof e === "object" && "message" in e && typeof (e as ShareError).message === "string") {
    return (e as ShareError).message;
  }
  return String(e);
}

/** What the updater is doing (`update_status`, event `update:status`). */
export type UpdatePhase =
  | { phase: "idle" }
  | { phase: "checking" }
  | { phase: "installing"; received: number; total: number }
  | { phase: "failed"; error: string };

export interface UpdateStatus {
  currentVersion: string;
  /** A newer version that exists, "" when none is known. */
  available: string;
  /** Unix seconds of the last successful check, 0 = never. */
  checkedAt: number;
  phase: UpdatePhase;
  /** First launch after an update: the popover says so once. */
  justUpdated: boolean;
}

/** Where to get the new version by hand when there is no automatic update. */
export const DOWNLOAD_URL = "https://socorin.com/#download";

export interface PlatformInfo {
  os: string;
  screenPermission: boolean;
  wayland: boolean;
  /** macOS: "disk-image" | "translocated" when this copy cannot be granted permissions. */
  installIssue: "disk-image" | "translocated" | null;
}

export interface DebugOptions {
  enabled: boolean;
  dumpDir: string | null;
  autoSelect: [number, number, number, number] | null;
  autoAction: string | null;
}

export const ipc = {
  debugOptions: () => invoke<DebugOptions>("debug_options"),
  /** Appends to the Rust diagnostics log (no-op unless SOCORIN_DEBUG is set). */
  debugLog: (message: string) => invoke<void>("debug_log", { message }),
  platformInfo: () => invoke<PlatformInfo>("platform_info"),
  requestScreenPermission: () => invoke<boolean>("request_screen_permission"),
  openScreenPermissionSettings: () => invoke<void>("open_screen_permission_settings"),
  startCapture: () => invoke<void>("start_capture"),
  startFullscreenCapture: () => invoke<void>("start_fullscreen_capture"),
  cancelCapture: () => invoke<void>("cancel_capture"),
  overlayInfo: (monitorId: number) => invoke<MonitorInfo>("overlay_info", { monitorId }),
  overlayPixels: (monitorId: number) => invoke<ArrayBuffer>("overlay_pixels", { monitorId }),
  /** The overlay has painted; Rust shows (and focuses) the window. */
  overlayReady: (monitorId: number) => invoke<void>("overlay_ready", { monitorId }),
  finishRegion: (region: Region) => invoke<void>("finish_region", { region }),
  /** The overlay switched to in-place annotation for the selected region. */
  beginAnnotation: (monitorId: number) => invoke<void>("begin_annotation", { monitorId }),
  /** Ends the capture session (hides the overlays); same as cancelling. */
  endCapture: () => invoke<void>("cancel_capture"),
  /**
   * Ends a running session, shows the system "Save as" dialog (in Rust: the
   * page never names a destination) and writes the PNG. Resolves to the
   * saved path, or null when the dialog was cancelled.
   */
  savePngAs: (png: Uint8Array) => invoke<string | null>("save_png_as", png),
  /** Ends the session and opens the PNG in the editor window. */
  editPng: (png: Uint8Array) => invoke<void>("edit_png", png),
  pendingImage: () => invoke<ArrayBuffer>("pending_image"),
  /** Saves PNG bytes to an auto-named file in the save dir. Returns the path. */
  savePng: (png: Uint8Array) => invoke<string>("save_png", png),
  copyPng: (png: Uint8Array) => invoke<void>("copy_png", png),
  /** Pick an area on the overlay and record it (stops a running recording). */
  startRecordRegion: () => invoke<void>("start_record_region"),
  startRecordFullscreen: () => invoke<void>("start_record_fullscreen"),
  /**
   * The overlay chose the area (bitmap pixels): end the session and record
   * it. `bar` is where the Record bar is, for the recording bar to take over.
   */
  startRecording: (region: Region, bar?: Bar) => invoke<void>("start_recording", { region, bar: bar ?? null }),
  /** Stop and reveal the file. */
  stopRecording: () => invoke<void>("stop_recording"),
  /** Stop and put the file on the clipboard. */
  stopRecordingCopy: () => invoke<void>("stop_recording_copy"),
  /** Stop, upload the file to the share server and put the link on the clipboard. */
  stopRecordingUpload: () => invoke<void>("stop_recording_upload"),
  /** Stop and delete the file. */
  cancelRecording: () => invoke<void>("cancel_recording"),
  recordingStatus: () => invoke<RecordingStatus>("recording_status"),
  getSettings: () => invoke<Settings>("get_settings"),
  /** Partial update: only the given keys change. Returns the resulting settings. */
  updateSettings: (patch: Partial<Settings>) => invoke<Settings>("update_settings", { patch }),
  /** The welcome page has rendered: show its window. */
  welcomeReady: () => invoke<void>("welcome_ready"),
  /** OK on the welcome dialog: remember it and close the window. */
  dismissWelcome: () => invoke<void>("dismiss_welcome"),
  defaultSaveDir: () => invoke<string>("default_save_dir"),
  openSaveDir: () => invoke<void>("open_save_dir"),
  showSettings: () => invoke<void>("show_settings"),
  updateStatus: () => invoke<UpdateStatus>("update_status"),
  /** Look for a new version right away; resolves once the check is done. */
  checkForUpdates: () => invoke<UpdateStatus>("check_for_updates"),
  /** Download, install and restart into the announced version. */
  installUpdate: () => invoke<void>("install_update"),
  /** Close the update popover (Later / OK / Escape). */
  dismissUpdate: () => invoke<void>("dismiss_update"),
  /** The update popover has rendered: show its window. */
  updateReady: () => invoke<void>("update_ready"),
  /**
   * Uploads PNG bytes to the share server, puts the link on the clipboard
   * and shows the popover. Rejects with a `ShareError`.
   */
  uploadPng: (png: Uint8Array) => invoke<ShareResult>("upload_png", png),
  /** The remembered links, newest first. */
  shareHistory: () => invoke<SharedLink[]>("share_history"),
  /** "Delete from server" for a remembered link. Rejects with a `ShareError`. */
  deleteShare: (id: string) => invoke<void>("delete_share", { id }),
  /** Put a remembered link on the clipboard again. */
  copyShareLink: (id: string) => invoke<void>("copy_share_link", { id }),
  /** Forget the install id; the next upload registers a new one. */
  resetInstallId: () => invoke<Settings>("reset_install_id"),
  shareNotice: () => invoke<ShareNotice | null>("share_notice"),
  /** The share popover has rendered: show its window. */
  shareReady: () => invoke<void>("share_ready"),
  /** Close the share popover. */
  dismissShare: () => invoke<void>("dismiss_share"),
  quit: () => invoke<void>("quit"),
};

export function timestamp(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}_${p(d.getHours())}-${p(d.getMinutes())}-${p(d.getSeconds())}`;
}
