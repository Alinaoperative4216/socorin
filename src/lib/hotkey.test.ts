import { afterEach, describe, expect, it, vi } from "vitest";

type HotkeyModule = typeof import("./hotkey");

/** Load the module fresh with the given platform (isMac is decided at import time). */
async function loadFor(platform: "mac" | "other"): Promise<HotkeyModule> {
  vi.resetModules();
  vi.stubGlobal("navigator", {
    platform: platform === "mac" ? "MacIntel" : "Win32",
    userAgent: platform === "mac" ? "Mozilla/5.0 (Macintosh)" : "Mozilla/5.0 (Windows NT 10.0)",
  });
  return import("./hotkey");
}

function key(init: Partial<KeyboardEventInit> & { code?: string; key?: string }): KeyboardEvent {
  return new KeyboardEvent("keydown", init);
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("shortcutFromEvent on macOS", () => {
  it("maps ⌘ to CmdOrCtrl and ⌃ to Ctrl", async () => {
    const { shortcutFromEvent, isMac } = await loadFor("mac");
    expect(isMac).toBe(true);
    expect(shortcutFromEvent(key({ code: "KeyA", key: "a", metaKey: true, shiftKey: true }))).toBe("CmdOrCtrl+Shift+A");
    expect(shortcutFromEvent(key({ code: "KeyB", key: "b", ctrlKey: true }))).toBe("Ctrl+B");
    expect(shortcutFromEvent(key({ code: "Digit3", key: "3", metaKey: true, altKey: true }))).toBe("CmdOrCtrl+Alt+3");
  });

  it("ignores modifier-only presses and unmodified letters", async () => {
    const { shortcutFromEvent } = await loadFor("mac");
    expect(shortcutFromEvent(key({ code: "ShiftLeft", key: "Shift", shiftKey: true }))).toBeNull();
    expect(shortcutFromEvent(key({ code: "MetaLeft", key: "Meta", metaKey: true }))).toBeNull();
    expect(shortcutFromEvent(key({ code: "KeyA", key: "a" }))).toBeNull();
    expect(shortcutFromEvent(key({ code: "KeyA", key: "a", shiftKey: true }))).toBe("Shift+A");
  });

  it("allows function keys and PrintScreen on their own", async () => {
    const { shortcutFromEvent } = await loadFor("mac");
    expect(shortcutFromEvent(key({ code: "F5", key: "F5" }))).toBe("F5");
    expect(shortcutFromEvent(key({ code: "F13", key: "F13", shiftKey: true }))).toBe("Shift+F13");
    expect(shortcutFromEvent(key({ code: "PrintScreen", key: "PrintScreen" }))).toBe("PrintScreen");
  });

  it("accepts named, numpad keys and rejects unknown codes", async () => {
    const { shortcutFromEvent } = await loadFor("mac");
    expect(shortcutFromEvent(key({ code: "Space", key: " ", metaKey: true }))).toBe("CmdOrCtrl+Space");
    expect(shortcutFromEvent(key({ code: "Numpad5", key: "5", metaKey: true }))).toBe("CmdOrCtrl+Numpad5");
    expect(shortcutFromEvent(key({ code: "BracketLeft", key: "[", ctrlKey: true }))).toBe("Ctrl+BracketLeft");
    expect(shortcutFromEvent(key({ code: "Unidentified", key: "Dead", metaKey: true }))).toBeNull();
  });

  it("reconstructs the code from the key when the event has none", async () => {
    const { shortcutFromEvent } = await loadFor("mac");
    expect(shortcutFromEvent(key({ key: "s", metaKey: true }))).toBe("CmdOrCtrl+S");
    expect(shortcutFromEvent(key({ key: "7", metaKey: true }))).toBe("CmdOrCtrl+7");
    expect(shortcutFromEvent(key({ key: "F2" }))).toBe("F2");
    expect(shortcutFromEvent(key({ key: "-", metaKey: true }))).toBe("CmdOrCtrl+Minus");
    expect(shortcutFromEvent(key({ key: "Escape", metaKey: true }))).toBe("CmdOrCtrl+Escape");
    expect(shortcutFromEvent(key({ key: "Dead", metaKey: true }))).toBeNull();
    expect(shortcutFromEvent(key({ key: "", metaKey: true }))).toBeNull();
  });
});

describe("shortcutFromEvent elsewhere", () => {
  it("maps Ctrl to CmdOrCtrl and the Windows key to Super", async () => {
    const { shortcutFromEvent, isMac } = await loadFor("other");
    expect(isMac).toBe(false);
    expect(shortcutFromEvent(key({ code: "KeyA", key: "a", ctrlKey: true, shiftKey: true }))).toBe("CmdOrCtrl+Shift+A");
    expect(shortcutFromEvent(key({ code: "KeyP", key: "p", metaKey: true }))).toBe("Super+P");
    expect(shortcutFromEvent(key({ code: "KeyP", key: "p", altKey: true }))).toBe("Alt+P");
  });
});

describe("prettyShortcut", () => {
  it("uses symbols on macOS", async () => {
    const { prettyShortcut } = await loadFor("mac");
    expect(prettyShortcut("CmdOrCtrl+Shift+A")).toBe("⌘⇧A");
    expect(prettyShortcut("Ctrl+Alt+ArrowUp")).toBe("⌃⌥↑");
    expect(prettyShortcut("Super+Enter")).toBe("⌘↩");
    expect(prettyShortcut("F5")).toBe("F5");
    expect(prettyShortcut("")).toBe("");
  });

  it("spells the modifiers out elsewhere", async () => {
    const { prettyShortcut } = await loadFor("other");
    expect(prettyShortcut("CmdOrCtrl+Shift+A")).toBe("Ctrl + Shift + A");
    expect(prettyShortcut("Super+PrintScreen")).toBe("Win + PrtSc");
    expect(prettyShortcut("Alt+Escape+")).toBe("Alt + Esc");
  });
});
