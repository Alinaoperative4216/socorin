import { screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flush, installTauri, SETTINGS, UPDATE_STATUS, type TauriMock } from "./test/tauri";

// main.tsx renders on import and keeps no handle to its React root, so the
// trees these tests boot would stay mounted after the test (the recording
// bar's clock keeps ticking) and React could schedule work once Vitest has
// torn jsdom down ("window is not defined"). Every root created is recorded
// here and unmounted after each test.
const roots = vi.hoisted(() => [] as { unmount(): void }[]);
vi.mock("react-dom/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("react-dom/client")>();
  const createRoot: typeof actual.createRoot = (...args) => {
    const root = actual.createRoot(...args);
    roots.push(root);
    return root;
  };
  const original = actual as typeof actual & { default?: object };
  return { ...actual, createRoot, default: { ...original.default, createRoot } };
});

let tauri: TauriMock;
let root: HTMLElement;

/** Boot the app as window `label` (main.tsx renders on import). */
async function boot(label: string, search = "") {
  window.history.replaceState({}, "", `/${search}`);
  tauri = installTauri(label, {
    get_settings: () => SETTINGS,
    platform_info: () => ({ os: "macos", screenPermission: true, wayland: false, installIssue: null }),
    overlay_info: () => Promise.reject("idle"),
    pending_image: () => Promise.reject("no image"),
    recording_status: () => ({ recording: false, startedMs: 0, path: "" }),
    debug_options: () => ({ enabled: false, dumpDir: null, autoSelect: null, autoAction: null }),
    update_status: () => ({ ...UPDATE_STATUS, available: "9.9.9" }),
  });
  vi.resetModules();
  await import("./main");
  await flush();
}

beforeEach(() => {
  root = document.createElement("div");
  root.id = "root";
  document.body.appendChild(root);
});

afterEach(async () => {
  for (const r of roots.splice(0)) r.unmount();
  await flush(); // let work React already queued run while jsdom still exists
  root.remove();
  window.history.replaceState({}, "", "/");
});

describe("main.tsx routing by window label", () => {
  it("mounts the overlay for overlay-* windows", async () => {
    await boot("overlay-3");
    expect(document.documentElement.dataset.window).toBe("overlay");
    await waitFor(() => expect(root.querySelector(".overlay")).toBeTruthy());
    await waitFor(() => expect(tauri.calls("overlay_info")).toEqual([{ monitorId: 3 }]));
  });

  it("mounts the editor", async () => {
    await boot("editor");
    expect(document.documentElement.dataset.window).toBe("editor");
    await screen.findByText("No screenshot to edit yet.");
  });

  it("mounts the welcome dialog", async () => {
    await boot("welcome");
    await screen.findByText("Welcome to Socorin");
    await waitFor(() => expect(tauri.calls("welcome_ready")).toHaveLength(1));
  });

  it("mounts the recording bar", async () => {
    await boot("recorder");
    await screen.findByTitle("Stop recording");
  });

  it("mounts the update popover", async () => {
    await boot("update");
    expect(document.documentElement.dataset.window).toBe("update");
    await screen.findByText("Socorin 9.9.9 is available");
    await waitFor(() => expect(tauri.calls("update_ready")).toHaveLength(1));
  });

  it("mounts the settings window for the main label", async () => {
    await boot("main");
    expect(document.documentElement.dataset.window).toBe("main");
    await screen.findByText("Shortcuts");
  });

  it("installs the browser mock when ?mock= is given in dev", async () => {
    const log = vi.spyOn(console, "log").mockImplementation(() => {});
    await boot("ignored", "?mock=main");
    // devmock is imported lazily: give it time to load.
    await waitFor(() => expect(log).toHaveBeenCalledWith('[mock] Tauri runtime mocked as window "main"'));
    await screen.findByText("Shortcuts");
    expect(tauri.invoke).not.toHaveBeenCalledWith("get_settings", expect.anything(), expect.anything());
  });
});
