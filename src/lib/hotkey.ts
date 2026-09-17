/** Helpers to record and display global shortcuts in Tauri's accelerator syntax. */

export const isMac = /Mac|iPhone|iPad/.test(navigator.platform) || /Macintosh/.test(navigator.userAgent);

const MODIFIER_CODES = new Set([
  "ShiftLeft", "ShiftRight", "ControlLeft", "ControlRight",
  "AltLeft", "AltRight", "MetaLeft", "MetaRight", "CapsLock", "Fn", "FnLock",
]);

const NAMED_KEYS = new Set([
  "Space", "Enter", "Escape", "Backspace", "Delete", "Tab", "Home", "End",
  "PageUp", "PageDown", "Insert", "PrintScreen", "ScrollLock", "Pause",
  "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight",
  "Minus", "Equal", "BracketLeft", "BracketRight", "Semicolon", "Quote",
  "Comma", "Period", "Slash", "Backslash", "Backquote",
]);

/**
 * Turn a keydown event into an accelerator such as "CmdOrCtrl+Shift+A".
 * Returns null for modifier-only presses or shortcuts that would hijack typing.
 */
const KEY_TO_CODE: Record<string, string> = {
  " ": "Space", Enter: "Enter", Escape: "Escape", Backspace: "Backspace", Delete: "Delete", Tab: "Tab",
  Home: "Home", End: "End", PageUp: "PageUp", PageDown: "PageDown", Insert: "Insert", PrintScreen: "PrintScreen",
  ArrowUp: "ArrowUp", ArrowDown: "ArrowDown", ArrowLeft: "ArrowLeft", ArrowRight: "ArrowRight",
  "-": "Minus", "=": "Equal", "[": "BracketLeft", "]": "BracketRight", ";": "Semicolon", "'": "Quote",
  ",": "Comma", ".": "Period", "/": "Slash", "\\": "Backslash", "`": "Backquote",
};

/** Some synthetic events carry no `code`; reconstruct it from `key`. */
function codeFromKey(key: string): string {
  if (/^[a-zA-Z]$/.test(key)) return `Key${key.toUpperCase()}`;
  if (/^[0-9]$/.test(key)) return `Digit${key}`;
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(key)) return key;
  return KEY_TO_CODE[key] ?? "";
}

export function shortcutFromEvent(e: KeyboardEvent): string | null {
  const code = e.code || codeFromKey(e.key);
  if (!code || MODIFIER_CODES.has(code)) return null;

  const mods: string[] = [];
  if (isMac ? e.metaKey : e.ctrlKey) mods.push("CmdOrCtrl");
  if (isMac && e.ctrlKey) mods.push("Ctrl");
  if (!isMac && e.metaKey) mods.push("Super");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");

  let key: string;
  if (/^Key[A-Z]$/.test(code)) key = code.slice(3);
  else if (/^Digit[0-9]$/.test(code)) key = code.slice(5);
  else if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) key = code;
  else if (/^Numpad/.test(code) || NAMED_KEYS.has(code)) key = code;
  else return null;

  const standalone = /^F\d+$/.test(key) || key === "PrintScreen";
  if (mods.length === 0 && !standalone) return null;
  return [...mods, key].join("+");
}

const MAC_SYMBOLS: Record<string, string> = {
  CmdOrCtrl: "⌘", Ctrl: "⌃", Alt: "⌥", Shift: "⇧", Super: "⌘",
  ArrowUp: "↑", ArrowDown: "↓", ArrowLeft: "←", ArrowRight: "→",
  Enter: "↩", Escape: "⎋", Backspace: "⌫", Delete: "⌦", Space: "␣",
};
const OTHER_NAMES: Record<string, string> = {
  CmdOrCtrl: "Ctrl", Super: "Win", ArrowUp: "↑", ArrowDown: "↓",
  ArrowLeft: "←", ArrowRight: "→", Escape: "Esc", PrintScreen: "PrtSc",
};

/** Human-friendly rendering of an accelerator string. */
export function prettyShortcut(shortcut: string): string {
  if (!shortcut) return "";
  const parts = shortcut.split("+").filter(Boolean);
  if (isMac) return parts.map((p) => MAC_SYMBOLS[p] ?? p).join("");
  return parts.map((p) => OTHER_NAMES[p] ?? p).join(" + ");
}
